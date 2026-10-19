//! `pitk fev` — extracción y reconciliación de ítems de un documento tabular
//! de ítems/precios (factura, cuenta de cobro, estado de cuenta…). `fev` es el
//! nombre del adaptador; sirve para cualquier PDF con filas de ítems y valores.
//!
//! Dos estilos de tabla observados en documentos reales:
//!
//! 1. **inline**: cada fila viene en una sola línea de texto con los valores al
//!    final (`1 332101 DESCRIPCION... 1.00 1,442,000.00 1,442,000.00`).
//! 2. **geometry**: la tabla se emite celda por celda (descripción partida,
//!    cantidad y valores en líneas separadas). Se reconstruye por bandas de y.
//!
//! En ambos casos se valida `cantidad × unitario ≈ total` por ítem y se
//! reconcilia la suma contra los totales declarados en el documento.

use anyhow::Result;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::model::Block;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FevItem {
    /// número de ítem tal como aparece (1..N, o por grupo)
    pub linea: usize,
    pub group: Option<String>,
    pub code1: String,
    pub code2: Option<String>,
    pub desc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autorizacion: Option<String>,
    pub qty: f64,
    pub vu: f64,
    pub vt: f64,
    pub page: usize,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FevGroup {
    pub name: String,
    pub items: usize,
    pub sum_vt: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declared: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reconciliation {
    pub items: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declared_lines: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declared_total: Option<f64>,
    pub sum_vt: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<f64>,
    pub arithmetic_ok: bool,
    pub ok: bool,
    pub duplicates: Vec<String>,
    pub missing: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FevReport {
    pub doc_id: String,
    pub file_name: String,
    pub tool_version: String,
    pub mode: String,
    pub items: Vec<FevItem>,
    pub groups: Vec<FevGroup>,
    pub reconciliation: Reconciliation,
    pub warnings: Vec<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// formato monetario colombiano / internacional
// ─────────────────────────────────────────────────────────────────────────────

/// `$ 1.234.567`, `1,442,000.00`, `987.654`, `1.00`, `$0`.
pub fn parse_money(raw: &str) -> Option<f64> {
    let t = raw.trim().trim_start_matches('$').trim().replace(' ', "");
    if t.is_empty() {
        return None;
    }
    if !t
        .chars()
        .all(|c| c.is_ascii_digit() || c == '.' || c == ',')
    {
        return None;
    }
    let last_dot = t.rfind('.');
    let last_comma = t.rfind(',');
    let normalized = match (last_dot, last_comma) {
        (Some(d), Some(c)) => {
            if d > c {
                t.replace(',', "") // 1,442,000.00
            } else {
                t.replace('.', "").replace(',', ".") // 1.442.000,00
            }
        }
        (Some(d), None) => {
            if t.len() - d - 1 == 2 {
                t.clone() // decimal
            } else {
                t.replace('.', "") // miles
            }
        }
        (None, Some(c)) => {
            if t.len() - c - 1 == 2 && c == t.len() - 3 {
                t.replace(',', ".")
            } else {
                t.replace(',', "")
            }
        }
        (None, None) => t.clone(),
    };
    normalized.parse::<f64>().ok()
}

/// ¿El token parece dinero? (`$...`, con separador de miles, o decimales)
pub fn looks_money(tok: &str) -> bool {
    let t = tok.trim();
    if t.starts_with('$') && t.len() > 1 {
        return true;
    }
    let s = t.trim_start_matches('$');
    if s.contains(',') && s.chars().any(|c| c.is_ascii_digit()) {
        return true;
    }
    // 987.654 / 13.319.081 / 1.442.000  (grupos de 3)
    if s.contains('.') {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() >= 2
            && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        {
            if parts.len() == 2 && parts[1].len() == 2 {
                return parts[0].len() <= 3; // 1.00 → sí; 162.74 → sí; 1000.00 ¿? no por len
            }
            return parts[0].len() <= 3 && parts[1..].iter().all(|p| p.len() == 3);
        }
    }
    false
}

fn qty_value(tok: &str) -> Option<f64> {
    let t = tok.trim().trim_end_matches(',');
    if !t.chars().all(|c| c.is_ascii_digit() || c == '.') || t.is_empty() {
        return None;
    }
    let v = t.parse::<f64>().ok()?;
    if (0.0..=100_000.0).contains(&v) {
        Some(v)
    } else {
        None
    }
}

fn code_like(tok: &str) -> bool {
    let t = tok.trim();
    (2..=25).contains(&t.len())
        && t.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '/')
        && t.chars().any(|c| c.is_ascii_digit())
}

fn parse_autorizacion(tok: &str) -> Option<String> {
    let t = tok.trim();
    if t.len() >= 6 && t.chars().all(|c| c.is_ascii_digit()) {
        Some(t.to_string())
    } else {
        None
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// parser principal
// ─────────────────────────────────────────────────────────────────────────────

pub fn parse_blocks(doc_id: &str, file_name: &str, blocks: &[Block]) -> FevReport {
    let anchor_re = Regex::new(r"^(\d{1,3})\s+([A-Za-z0-9][\w\-./]{1,24})(\s+|$)").unwrap();
    let total_re =
        Regex::new(r"(?i)(valor total a pagar|total facturado|total a pagar|subtotal|total grupo)").unwrap();
    let declared_lines_re = Regex::new(r"(?i)total\s+l[ií]neas\s*:?\s*(\d+)").unwrap();

    // 1) detectar líneas ancla (número de ítem + código)
    let date_re = Regex::new(r"^\d{4}[-/ ]\d{1,2}[-/ ]\d{1,2}").unwrap();
    let time_re = Regex::new(r"^\d{1,2}:\d{2}").unwrap();
    let mut anchors: Vec<usize> = Vec::new();
    for (i, b) in blocks.iter().enumerate() {
        let t = b.text.trim();
        if anchor_re.is_match(t)
            && !is_header_or_footer(t)
            && !date_re.is_match(t)
            && !time_re.is_match(t)
        {
            anchors.push(i);
        }
    }

    // 2) ¿modo inline? contamos anclas cuyo final trae (qty, vu, vt) válidos
    let inline_matches = anchors
        .iter()
        .filter(|&&i| parse_inline(&blocks[i]).is_some())
        .count();
    if std::env::var("PITK_DEBUG").is_ok() {
        eprintln!("[dbg] anchors={} inline_matches={}", anchors.len(), inline_matches);
    }
    let mode = if inline_matches >= 10 && inline_matches * 2 >= anchors.len() {
        "inline"
    } else {
        "geometry"
    };

    // 3) detectar grupos ("GRUPO: X") y totales declarados
    #[derive(Clone, Copy)]
    enum Pending {
        Group,
        Doc,
    }
    let mut group_of_block: Vec<Option<String>> = vec![None; blocks.len()];
    let mut current_group: Option<String> = None;
    let mut declared_group: BTreeMap<String, f64> = BTreeMap::new();
    let mut declared_total: Option<f64> = None;
    let mut declared_lines: Option<usize> = None;
    let mut pending: Option<Pending> = None;

    for (i, b) in blocks.iter().enumerate() {
        let text = b.text.trim();
        group_of_block[i] = current_group.clone();

        if let Some(caps) = declared_lines_re.captures(text) {
            if let Ok(n) = caps[1].parse::<usize>() {
                declared_lines = Some(n);
            }
        }

        if let Some(name) = group_name(text) {
            current_group = Some(name);
            group_of_block[i] = current_group.clone();
            pending = None;
            continue;
        }

        let lower = text.to_lowercase();
        if lower.starts_with("total grupo") {
            if let Some(v) = first_money(text) {
                if let Some(g) = &current_group {
                    declared_group.insert(g.clone(), v);
                }
            } else {
                pending = Some(Pending::Group);
            }
            continue;
        }

        if total_re.is_match(text) {
            if let Some(v) = first_money(text) {
                declared_total = Some(v);
            } else {
                pending = Some(Pending::Doc);
            }
            continue;
        }

        if let Some(kind) = pending {
            if let Some(v) = money_only(text) {
                match kind {
                    Pending::Doc => declared_total = Some(v),
                    Pending::Group => {
                        if let Some(g) = &current_group {
                            declared_group.insert(g.clone(), v);
                        }
                    }
                }
                pending = None;
            }
        }
    }

    // 4) construir ítems
    let mut items: Vec<FevItem> = Vec::new();
    match mode {
        "inline" => {
            let mut last: Option<usize> = None;
            for (i, b) in blocks.iter().enumerate() {
                if let Some(item) = parse_inline(b) {
                    let mut item = item;
                    item.page = b.page;
                    item.group = group_of_block[i].clone();
                    items.push(item);
                    last = Some(items.len() - 1);
                } else if let Some(li) = last {
                    // continuación de descripción de la fila anterior
                    let t = b.text.trim();
                    if is_description_continuation(t) && !is_header_or_footer(t) {
                        if !items[li].desc.is_empty() {
                            items[li].desc.push(' ');
                        }
                        items[li].desc.push_str(t);
                    }
                }
            }
        }
        _ => {
            // bandas de y entre anclas, por página
            for (ai, &ai_idx) in anchors.iter().enumerate() {
                let anchor = &blocks[ai_idx];
                if anchor.page != blocks[ai_idx].page {
                    continue;
                }
                let y_top = anchor.bbox.map(|b| b[1]).unwrap_or(0.0);
                let next = anchors.get(ai + 1).map(|&j| &blocks[j]);
                let same_page_next = next.filter(|n| n.page == anchor.page);
                let y_bottom = same_page_next
                    .and_then(|n| n.bbox.map(|b| b[1]))
                    .unwrap_or(f32::MIN);

                // La fila de cierre del grupo ("Total Grupo") no pertenece al ítem:
                // recortamos la banda justo antes de ella.
                let cutoff_y = blocks
                    .iter()
                    .filter(|b| b.page == anchor.page)
                    .filter(|b| {
                        let t = b.text.trim().to_uppercase();
                        (t.starts_with("TOTAL GRUPO")
                            || t.starts_with("SUBTOTAL")
                            || total_re.is_match(&b.text))
                            && b.bbox.map(|bb| bb[1] < y_top + 1.5).unwrap_or(false)
                            && b.bbox.map(|bb| bb[1] > y_bottom).unwrap_or(false)
                    })
                    .filter_map(|b| b.bbox.map(|bb| bb[1]))
                    .fold(f32::MIN, f32::max);
                let cutoff_y = if cutoff_y > f32::MIN { cutoff_y } else { y_bottom };

                let band: Vec<&Block> = blocks
                    .iter()
                    .filter(|b| {
                        if b.page != anchor.page {
                            return false;
                        }
                        match b.bbox {
                            Some(bb) => bb[1] <= y_top + 1.5 && bb[1] > cutoff_y,
                            None => false,
                        }
                    })
                    .collect();

                if let Some(item) = build_item_from_band(anchor, &band, group_of_block[ai_idx].clone()) {
                    items.push(item);
                }
            }
        }
    }

    // 5) reconciliación
    let sum_vt: f64 = items.iter().map(|i| i.vt).sum();
    let mut warnings: Vec<String> = Vec::new();

    for it in &items {
        let expected = it.qty * it.vu;
        if (expected - it.vt).abs() > (0.5f64).max(it.vt * 0.0005) {
            warnings.push(format!(
                "ítem {} p{}: {} × {} ≠ {} (diferencia {:.2})",
                it.linea,
                it.page,
                it.qty,
                it.vu,
                it.vt,
                it.vt - expected
            ));
        }
    }

    // duplicados / faltantes por grupo (los ítems se numeran por grupo)
    let mut duplicates: Vec<String> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    let mut by_group: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for it in &items {
        by_group
            .entry(it.group.clone().unwrap_or_else(|| "(sin grupo)".into()))
            .or_default()
            .push(it.linea);
    }
    for (g, nums) in &by_group {
        let mut seen = std::collections::HashSet::new();
        for n in nums {
            if !seen.insert(*n) {
                duplicates.push(format!("{g}: ítem {n} repetido"));
            }
        }
        let max = nums.iter().copied().max().unwrap_or(0);
        for n in 1..=max {
            if !seen.contains(&n) {
                missing.push(format!("{g}: falta ítem {n}"));
            }
        }
    }

    let arithmetic_ok = warnings.is_empty();
    let diff = declared_total.map(|t| sum_vt - t);
    let total_ok = match (declared_total, diff) {
        (Some(_), Some(d)) => d.abs() <= 0.5f64.max(sum_vt * 0.000001),
        _ => false,
    };
    let lines_ok = match declared_lines {
        Some(n) => n == items.len(),
        None => true,
    };

    if let Some(n) = declared_lines {
        if n != items.len() {
            warnings.push(format!(
                "el documento declara {} líneas y se extrajeron {}",
                n,
                items.len()
            ));
        }
    }
    if let Some(t) = declared_total {
        if !total_ok {
            warnings.push(format!(
                "suma de ítems {:.2} ≠ total declarado {:.2} (diferencia {:+.2})",
                sum_vt,
                t,
                sum_vt - t
            ));
        }
    }
    if declared_total.is_none() {
        warnings.push("no se detectó total declarado en el documento".to_string());
    }

    // grupos con suma y total declarado
    let mut groups: Vec<FevGroup> = Vec::new();
    for (g, nums) in &by_group {
        let sum: f64 = items
            .iter()
            .filter(|i| i.group.as_deref().unwrap_or("(sin grupo)") == g.as_str())
            .map(|i| i.vt)
            .sum();
        let declared = declared_group.get(g).copied();
        let diff = declared.map(|d| sum - d);
        if let Some(d) = diff {
            if d.abs() > 0.5f64.max(sum * 0.000001) {
                warnings.push(format!(
                    "grupo {}: suma {:.2} ≠ total de grupo declarado {:.2}",
                    g, sum, d + sum
                ));
            }
        }
        groups.push(FevGroup {
            name: g.clone(),
            items: nums.len(),
            sum_vt: round2(sum),
            declared: declared.map(round2),
            diff: diff.map(round2),
        });
    }

    let ok = arithmetic_ok && total_ok && lines_ok && duplicates.is_empty();
    let items_len = items.len();

    FevReport {
        doc_id: doc_id.to_string(),
        file_name: file_name.to_string(),
        tool_version: crate::store::TOOL_VERSION.to_string(),
        mode: mode.to_string(),
        items,
        groups,
        reconciliation: Reconciliation {
            items: items_len,
            declared_lines,
            declared_total: declared_total.map(round2),
            sum_vt: round2(sum_vt),
            diff: diff.map(round2),
            arithmetic_ok,
            ok,
            duplicates,
            missing,
        },
        warnings,
    }
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn first_money(text: &str) -> Option<f64> {
    text.split_whitespace()
        .filter(|t| looks_money(t))
        .filter_map(parse_money)
        .next_back()
}

fn money_only(text: &str) -> Option<f64> {
    let t = text.trim();
    let s = t.trim_start_matches('$').trim();
    if s.is_empty() || s.split_whitespace().count() > 1 {
        return None;
    }
    if t.starts_with('$') || looks_money(s) {
        parse_money(s)
    } else {
        None
    }
}

fn group_name(text: &str) -> Option<String> {
    let t = text.trim();
    let up = t.to_uppercase();
    for (prefix, rest) in [
        ("GRUPO:", ""),
        ("GRUPO ", ""),
        ("GRUPOS:", ""),
    ] {
        if let Some(r) = up.strip_prefix(prefix) {
            let _ = rest;
            let name = r.trim();
            if !name.is_empty() && name.chars().count() <= 40 {
                return Some(name.to_string());
            }
        }
    }
    // "GRUPO: PROCEDIMIENTOS" con prefijo exacto
    if up.starts_with("GRUPO:") {
        let name = t[6..].trim();
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    None
}

fn is_header_or_footer(text: &str) -> bool {
    let up = text.to_uppercase();
    up.contains("AUTORIZACIÓN CANTIDAD")
        || up.contains("AUTORIZACION CANTIDAD")
        || up.contains("FACTURA ELECTRÓNICA DE VENTA")
        || up.contains("FACTURA ELECTRONICA DE VENTA")
        || up.contains("REPRESENTACIÓN GRÁFICA")
        || up.contains("REPRESENTACION GRAFICA")
        || up.starts_with("PÁGINA ")
        || up.starts_with("PAGINA ")
        || up.starts_with("PÁG ")
        || up.starts_with("PAG ")
        || up.starts_with("SOFTWARE SIISA")
        || up.starts_with("CUFE:")
        || up.contains("TOTAL LÍNEAS")
        || up.contains("TOTAL LINEAS")
}

fn is_description_continuation(text: &str) -> bool {
    let t = text.trim();
    if t.len() < 2 || t.len() > 90 {
        return false;
    }
    // no es cabecera de tabla ni fila de valores
    if looks_money(&t) || qty_value(&t).is_some() {
        return false;
    }
    let up = t.to_uppercase();
    if up.contains("OBSERVACIONES") || up.starts_with("TOTAL") || up.starts_with("SUBTOTAL") {
        return false;
    }
    true
}

/// Fila completa en una sola línea: `N COD [COD2] DESC... CANT VU VT`.
fn parse_inline(b: &Block) -> Option<FevItem> {
    let text = b.text.trim();
    let toks: Vec<&str> = text.split_whitespace().collect();
    if toks.len() < 6 {
        return None;
    }
    let linea: usize = toks[0].parse().ok()?;
    if !code_like(toks[1]) {
        return None;
    }
    // últimos tres tokens: cantidad, unitario, total
    let n = toks.len();
    let tol = |vt: f64| (0.5f64).max(vt * 0.0005);

    // Cola numérica: la cantidad/valores pueden venir en orden inesperado
    // (columnas intercaladas por la extracción). Extraemos la cola numérica
    // y elegimos (qty, vu, vt) por aritmética: qty × vu ≈ vt.
    let mut tail_start = n;
    while tail_start > 2 {
        let t = toks[tail_start - 1];
        if looks_money(t) || qty_value(t).is_some() {
            tail_start -= 1;
        } else {
            break;
        }
    }
    if n - tail_start < 3 {
        return None;
    }
    let tail = &toks[tail_start..n];
    let (qty, vu, vt) = parse_tail(tail)?;

    let cut = tail_start;
    let mut code2 = None;
    let mut desc_tokens: &[&str] = &toks[2..cut];
    if let Some(&t2) = toks.get(2) {
        if code_like(t2) {
            code2 = Some(t2.to_string());
            desc_tokens = &toks[3..cut];
        }
    }
    let mut autorizacion = None;
    if let Some(&last) = desc_tokens.last() {
        if let Some(a) = parse_autorizacion(last) {
            autorizacion = Some(a);
            desc_tokens = &desc_tokens[..desc_tokens.len() - 1];
        }
    }
    let desc = desc_tokens.join(" ");
    if desc.is_empty() {
        return None;
    }

    Some(FevItem {
        linea,
        group: None,
        code1: toks[1].to_string(),
        code2,
        desc,
        autorizacion,
        qty,
        vu,
        vt,
        page: b.page,
        source: "inline".to_string(),
    })
}

fn is_value_marker(tok: &str) -> bool {
    matches!(
        tok.trim().to_uppercase().as_str(),
        "NO" | "APLICA" | "COMPARTIDO"
    )
}

fn parse_tail(tail: &[&str]) -> Option<(f64, f64, f64)> {
    let tol = |vt: f64| (0.5f64).max(vt * 0.0005);

    // orden canónico: qty, vu[, imp], vt
    if tail.len() >= 3 {
        if let (Some(q), Some(vu), Some(vt)) = (
            qty_value(tail[0]),
            parse_money(tail[1]),
            parse_money(tail[tail.len() - 1]),
        ) {
            if (q * vu - vt).abs() <= tol(vt) && q > 0.0 {
                return Some((q, vu, vt));
            }
        }
    }
    // un número de la descripción ("REF 1") pudo colarse al inicio de la cola
    if tail.len() >= 4 {
        if let (Some(q), Some(vu), Some(vt)) = (
            qty_value(tail[1]),
            parse_money(tail[2]),
            parse_money(tail[tail.len() - 1]),
        ) {
            if (q * vu - vt).abs() <= tol(vt) && q > 0.0 {
                return Some((q, vu, vt));
            }
        }
    }
    // 4 columnas con %IMP: qty, vu, imp, vt
    if tail.len() >= 4 {
        if let (Some(q), Some(vu), Some(vt)) = (
            qty_value(tail[0]),
            parse_money(tail[1]),
            parse_money(tail[3]),
        ) {
            if (q * vu - vt).abs() <= tol(vt) && q > 0.0 {
                return Some((q, vu, vt));
            }
        }
    }

    // orden intercalado: buscar vt (mayor valor), qty (pequeña) y vu que cuadre
    let values: Vec<f64> = tail.iter().filter_map(|t| parse_money(t)).collect();
    let vt = values.iter().copied().fold(f64::MIN, f64::max);
    if !vt.is_finite() || vt <= 0.0 {
        return None;
    }
    for (qi, qtok) in tail.iter().enumerate() {
        let q = match qty_value(qtok) {
            Some(q) if q > 0.0 => q,
            _ => continue,
        };
        for (vi, vtok) in tail.iter().enumerate() {
            if vi == qi {
                continue;
            }
            if let Some(vu) = parse_money(vtok) {
                if vu > 0.0 && (q * vu - vt).abs() <= tol(vt) {
                    return Some((q, vu, vt));
                }
            }
        }
    }
    None
}

/// Reconstruye un ítem desde la banda de líneas de su fila (modo geometry).
fn build_item_from_band(
    anchor: &Block,
    band: &[&Block],
    group: Option<String>,
) -> Option<FevItem> {
    let toks: Vec<&str> = anchor.text.split_whitespace().collect();
    if toks.len() < 2 {
        return None;
    }
    let linea: usize = toks[0].parse().ok()?;
    let code1 = toks[1].to_string();
    if !code_like(&code1) {
        return None;
    }

    let mut code2 = None;
    let mut desc_parts: Vec<String> = Vec::new();
    let mut autorizacion = None;
    let mut money: Vec<f64> = Vec::new();
    let mut qty_candidates: Vec<f64> = Vec::new();

    // cola numérica del propio ancla (fila completa en una línea)
    let mut tail_start = toks.len();
    while tail_start > 2
        && (looks_money(toks[tail_start - 1])
            || qty_value(toks[tail_start - 1]).is_some()
            || is_value_marker(toks[tail_start - 1]))
    {
        tail_start -= 1;
    }
    let anchor_tail = if toks.len() - tail_start >= 3 {
        parse_tail(&toks[tail_start..])
    } else {
        None
    };

    // texto del ancla
    let mut idx = 2;
    if let Some(&t2) = toks.get(2) {
        if code_like(t2) {
            code2 = Some(t2.to_string());
            idx = 3;
        }
    }
    let inner = &toks[idx..tail_start];
    let mut inner_used = vec![false; inner.len()];
    for i in 0..inner.len() {
        let t = inner[i];
        if let Some(a) = parse_autorizacion(t) {
            autorizacion = Some(a);
            inner_used[i] = true;
            continue;
        }
        if looks_money(t) {
            if let Some(v) = parse_money(t) {
                money.push(v);
                inner_used[i] = true;
                // "… 3 $13.319.081 …": el token numérico previo es la cantidad
                if i > 0 && !inner_used[i - 1] {
                    if let Some(q) = qty_value(inner[i - 1]) {
                        if q > 0.0 && q <= 100_000.0 && !looks_money(inner[i - 1]) {
                            qty_candidates.push(q);
                            inner_used[i - 1] = true;
                        }
                    }
                }
            }
        }
    }
    for (i, t) in inner.iter().enumerate() {
        if inner_used[i] {
            continue;
        }
        // restos de la fila de valores: no van a la descripción
        let clean = t.trim().to_uppercase();
        if is_value_marker(t) {
            continue;
        }
        desc_parts.push(t.to_string());
    }

    // resto de la banda
    for b in band {
        if std::ptr::eq(*b, anchor) {
            continue;
        }
        let text = b.text.trim();
        if is_header_or_footer(text) || text.to_uppercase().starts_with("TOTAL GRUPO") {
            continue;
        }
        let btoks: Vec<&str> = text.split_whitespace().collect();
        if btoks.is_empty() {
            continue;
        }
        // línea de valores: "1 $162.741 NO APLICA", "$0", "1"
        let mut used = vec![false; btoks.len()];
        for (i, t) in btoks.iter().enumerate() {
            if looks_money(t) {
                if anchor_tail.is_none() {
                    if let Some(v) = parse_money(t) {
                        money.push(v);
                        used[i] = true;
                    }
                } else {
                    used[i] = true; // los valores ya vienen del ancla
                }
            } else if let Some(a) = parse_autorizacion(t) {
                if autorizacion.is_none() {
                    autorizacion = Some(a);
                }
                used[i] = true;
            }
        }
        if btoks.len() == 1 && qty_value(btoks[0]).is_some() && !looks_money(btoks[0]) {
            qty_candidates.push(qty_value(btoks[0]).unwrap());
            used[0] = true;
        } else if let Some(q) = qty_value(btoks[0]) {
            if !looks_money(btoks[0]) && money_value_at_least_one(&btoks, 1) {
                qty_candidates.push(q);
                used[0] = true;
            }
        }
        let leftover: Vec<&str> = btoks
            .iter()
            .enumerate()
            .filter(|(i, _)| !used[*i])
            .map(|(_, t)| *t)
            .collect();
        let leftover_text = leftover.join(" ");
        let clean = leftover_text
            .trim()
            .trim_matches(|c: char| c == ',' || c == '-' || c == '.')
            .trim();
        if !clean.is_empty()
            && !clean.eq_ignore_ascii_case("NO APLICA")
            && !clean.eq_ignore_ascii_case("COMPARTIDO")
            && !clean.contains("NO APLICA")
        {
            desc_parts.push(clean.to_string());
        }
    }

    let (qty, vu, vt) = match anchor_tail {
        Some((q, vu, vt)) => (q, vu, vt),
        None => {
            if money.is_empty() {
                return None;
            }
            let vt = money.iter().copied().fold(f64::MIN, f64::max);
            let qty = choose_qty(&qty_candidates, &money, vt);
            let vu = money
                .iter()
                .copied()
                .find(|m| (qty * m - vt).abs() <= (0.5f64).max(vt * 0.0005))
                .unwrap_or_else(|| if qty > 0.0 { vt / qty } else { 0.0 });
            (qty, vu, vt)
        }
    };

    let desc = desc_parts
        .join(" ")
        .replace(" NO APLICA", "")
        .replace("NO APLICA ", "")
        .trim()
        .to_string();
    if desc.is_empty() {
        return None;
    }

    Some(FevItem {
        linea,
        group,
        code1,
        code2,
        desc,
        autorizacion,
        qty,
        vu: round2(vu),
        vt: round2(vt),
        page: anchor.page,
        source: "geometry".to_string(),
    })
}

fn money_value_at_least_one(toks: &[&str], from: usize) -> bool {
    toks.iter().skip(from).any(|t| looks_money(t))
}

fn choose_qty(cands: &[f64], money: &[f64], vt: f64) -> f64 {
    // primera cantidad q tal que q*m ≈ vt para algún m del dinero
    for q in cands {
        if *q <= 0.0 {
            continue;
        }
        if money
            .iter()
            .any(|m| (*q * m - vt).abs() <= (0.5f64).max(vt * 0.0005))
        {
            return *q;
        }
    }
    // si no, la cantidad más común (o 1)
    let mut best = 1.0;
    let mut best_score = 0usize;
    for q in cands {
        let score = cands.iter().filter(|x| (*x - q).abs() < 1e-9).count();
        if score > best_score {
            best = *q;
            best_score = score;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(id: &str, page: usize, y: f32, text: &str) -> Block {
        Block {
            id: id.to_string(),
            node: "n0".into(),
            page,
            kind: "line".into(),
            source: None,
            bbox: Some([20.0, y, 500.0, y + 8.0]),
            font_size: Some(8.0),
            text: text.to_string(),
        }
    }

    #[test]
    fn money_formats() {
        assert_eq!(parse_money("$ 1.234.567"), Some(1234567.0));
        assert_eq!(parse_money("1,442,000.00"), Some(1442000.0));
        assert_eq!(parse_money("987.654"), Some(987654.0));
        assert_eq!(parse_money("$0"), Some(0.0));
        assert_eq!(parse_money("1.00"), Some(1.0));
        assert_eq!(parse_money("2.800"), Some(2800.0));
        assert_eq!(parse_money("abc"), None);
    }

    #[test]
    fn inline_row() {
        let b = block(
            "b1",
            1,
            500.0,
            "1 332101 SERVICIO DE EJEMPLO UNO-PAQUETES-UND:NAR 1.00 1,442,000.00 1,442,000.00",
        );
        let it = parse_inline(&b).unwrap();
        assert_eq!(it.linea, 1);
        assert_eq!(it.code1, "332101");
        assert_eq!(it.qty, 1.0);
        assert_eq!(it.vu, 1442000.0);
        assert_eq!(it.vt, 1442000.0);
        assert!(it.desc.contains("SERVICIO DE EJEMPLO"));
    }

    #[test]
    fn geometry_row_band() {
        let anchor = block(
            "a",
            1,
            590.0,
            "1 992511 992511 SERVICIO DE EJEMPLO DOS 99999999",
        );
        let b1 = block("b1", 1, 570.0, "1 $987.654 NO APLICA");
        let b2 = block("b2", 1, 560.0, "$0");
        let b3 = block("b3", 1, 550.0, "$987.654");
        let b4 = block("b4", 1, 540.0, "DE EJEMPLO");
        let band = vec![&anchor, &b1, &b2, &b3, &b4];
        let it = build_item_from_band(&anchor, &band, Some("PROCEDIMIENTOS".into())).unwrap();
        assert_eq!(it.linea, 1);
        assert_eq!(it.qty, 1.0);
        assert_eq!(it.vu, 987654.0);
        assert_eq!(it.vt, 987654.0);
        assert!(it.desc.contains("SERVICIO DE EJEMPLO"));
        assert!(it.desc.contains("DE EJEMPLO"));
        assert_eq!(it.autorizacion.as_deref(), Some("99999999"));
    }

    #[test]
    fn geometry_row_fusionada() {
        // fila completa en UNA línea (lo que produce el agrupador de spans)
        let anchor = block(
            "a",
            2,
            463.0,
            "1 14932 SERVICIO DE EJEMPLO TRES 99999999 1 $39.600 NO APLICA $0 $39.600",
        );
        let band = vec![&anchor];
        let it =
            build_item_from_band(&anchor, &band, Some("DISPOSITIVO MEDICO".into())).unwrap();
        assert_eq!(it.linea, 1);
        assert_eq!(it.qty, 1.0);
        assert_eq!(it.vt, 39600.0);
        assert_eq!(it.vu, 39600.0);
        assert_eq!(it.autorizacion.as_deref(), Some("99999999"));
        assert!(it.desc.contains("SERVICIO DE EJEMPLO TRES"), "desc: {}", it.desc);
        assert!(!it.desc.contains("APLICA"), "desc con restos: {}", it.desc);
    }

    #[test]
    fn geometry_row_fusionada_qty_mayor() {
        let anchor = block(
            "a",
            2,
            304.0,
            "2 13994 02IN044002 INSUMO DE EJEMPLO 10 ML. EN 99999999 2 $400 NO APLICA $0 $800",
        );
        let band = vec![&anchor];
        let it = build_item_from_band(&anchor, &band, None).unwrap();
        assert_eq!(it.qty, 2.0, "la cantidad debe ser 2, no 10 (de '10 ML')");
        assert_eq!(it.vu, 400.0);
        assert_eq!(it.vt, 800.0);
        assert!(it.desc.contains("INSUMO DE EJEMPLO 10 ML"), "desc: {}", it.desc);
        assert!(!it.desc.contains("NO APLICA"));
    }

    #[test]
    fn reconcile_simple() {
        let blocks = vec![
            block("h", 1, 700.0, "GRUPO: PROCEDIMIENTOS"),
            block(
                "i1",
                1,
                680.0,
                "1 111111 SERVICIO UNO 2.00 1,000.00 2,000.00",
            ),
            block(
                "i2",
                1,
                660.0,
                "2 222222 SERVICIO DOS 1.00 500.00 500.00",
            ),
            block("t", 1, 640.0, "TOTAL A PAGAR 2,500.00"),
        ];
        let r = parse_blocks("doc", "f.pdf", &blocks);
        assert_eq!(r.items.len(), 2);
        assert_eq!(r.reconciliation.declared_total, Some(2500.0));
        assert_eq!(r.reconciliation.sum_vt, 2500.0);
        assert!(r.reconciliation.ok);
    }
}
