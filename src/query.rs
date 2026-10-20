//! Consultas v1b: índice de fechas (`dates`) y búsqueda léxica con recibo
//! (`search --receipt`).
//!
//! - `dates`: construye un índice ordenado fecha → páginas y responde con
//!   búsqueda binaria O(log n). Pensado para documentos cronológicos largos
//!   (p. ej. un expediente de 1.378 páginas): "¿qué pasó el 2026-09-02?".
//! - `search`: busca términos sobre `blocks.jsonl` y, con `--receipt`, produce
//!   un registro reproducible (hash del corpus incluido) que sirve como prueba
//!   de los términos probados en una verificación de ausencia.

use anyhow::{bail, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

use crate::model::Block;
use std::path::{Path, PathBuf};

pub const SCHEMA_DATES: &str = "pitk/dates/1";

// ─────────────────────────────────────────────────────────────────────────────
// fechas
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DateEntry {
    /// YYYY-MM-DD
    pub date: String,
    /// páginas (1-based), ordenadas
    pub pages: Vec<usize>,
    /// ocurrencias totales
    pub hits: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatesIndex {
    pub schema: String,
    pub doc_id: String,
    pub source_sha256: String,
    pub dates: Vec<DateEntry>,
}

pub fn build_dates_index(doc_id: &str, source_sha256: &str, blocks: &[Block]) -> DatesIndex {
    let re_dmy = Regex::new(r"\b(\d{1,2})[/-](\d{1,2})[/-](\d{4})\b").unwrap();
    let re_ymd = Regex::new(r"\b(\d{4})-(\d{2})-(\d{2})\b").unwrap();
    // "23 de julio del 2026"
    let re_larga = Regex::new(
        r"(?i)\b(\d{1,2})\s+de\s+(enero|febrero|marzo|abril|mayo|junio|julio|agosto|septiembre|setiembre|octubre|noviembre|diciembre)\s+(?:del?\s+)?(\d{4})\b",
    )
    .unwrap();
    // "23-JUL-2026", "23/JUL/2026", "23 jul 2026"
    let re_abrev = Regex::new(
        r"(?i)\b(\d{1,2})[-/ ](ene|feb|mar|abr|may|jun|jul|ago|sep|set|oct|nov|dic)[-/ ](\d{4})\b",
    )
    .unwrap();

    let mut map: std::collections::BTreeMap<String, (BTreeSet<usize>, usize)> =
        std::collections::BTreeMap::new();

    for b in blocks {
        let mut found: Vec<String> = Vec::new();
        for caps in re_dmy.captures_iter(&b.text) {
            let (d, m, y) = (&caps[1], &caps[2], &caps[3]);
            if let Some(iso) = normalize_dmy(d, m, y) {
                found.push(iso);
            }
        }
        for caps in re_ymd.captures_iter(&b.text) {
            if let Some(iso) = normalize_ymd(&caps[1], &caps[2], &caps[3]) {
                found.push(iso);
            }
        }
        for caps in re_larga.captures_iter(&b.text) {
            let m = month_number(&caps[2]).unwrap_or(0);
            if let Some(iso) = normalize_dmy(&caps[1], &m.to_string(), &caps[3]) {
                found.push(iso);
            }
        }
        for caps in re_abrev.captures_iter(&b.text) {
            let m = month_number(&caps[2]).unwrap_or(0);
            if let Some(iso) = normalize_dmy(&caps[1], &m.to_string(), &caps[3]) {
                found.push(iso);
            }
        }
        for iso in found {
            let e = map.entry(iso).or_default();
            e.0.insert(b.page);
            e.1 += 1;
        }
    }

    let dates = map
        .into_iter()
        .map(|(date, (pages, hits))| DateEntry {
            date,
            pages: pages.into_iter().collect(),
            hits,
        })
        .collect();

    DatesIndex {
        schema: SCHEMA_DATES.to_string(),
        doc_id: doc_id.to_string(),
        source_sha256: source_sha256.to_string(),
        dates,
    }
}

fn month_number(name: &str) -> Option<u32> {
    let n = name.to_lowercase();
    Some(match n.as_str() {
        "enero" | "ene" => 1,
        "febrero" | "feb" => 2,
        "marzo" | "mar" => 3,
        "abril" | "abr" => 4,
        "mayo" | "may" => 5,
        "junio" | "jun" => 6,
        "julio" | "jul" => 7,
        "agosto" | "ago" => 8,
        "septiembre" | "setiembre" | "sep" | "set" => 9,
        "octubre" | "oct" => 10,
        "noviembre" | "nov" => 11,
        "diciembre" | "dic" => 12,
        _ => return None,
    })
}

fn normalize_dmy(d: &str, m: &str, y: &str) -> Option<String> {
    let (d, m, y): (u32, u32, i32) = (d.parse().ok()?, m.parse().ok()?, y.parse().ok()?);
    if !(1..=31).contains(&d) || !(1..=12).contains(&m) || !(1990..=2100).contains(&y) {
        return None;
    }
    let date = chrono::NaiveDate::from_ymd_opt(y, m, d)?;
    Some(date.format("%Y-%m-%d").to_string())
}

fn normalize_ymd(y: &str, m: &str, d: &str) -> Option<String> {
    normalize_dmy(d, m, y)
}

/// Parsea `2026-09-02`, `02/09/2026`, `02-09-2026`, `2026-09` (mes completo)
/// y rangos `A..B`.
pub fn parse_date_query(q: &str) -> Result<(String, String)> {
    let q = q.trim();
    if let Some((a, b)) = q.split_once("..") {
        let start = parse_single(a.trim())?;
        let end = parse_single(b.trim())?;
        if start > end {
            bail!("rango invertido: {} > {}", start, end);
        }
        return Ok((start, end));
    }
    let single = parse_single(q)?;
    // mes (YYYY-MM) → rango del mes completo
    if is_month(q) {
        let (y, m) = (single[0..4].to_string(), single[5..7].to_string());
        let last = last_day_of_month(&y, &m)?;
        return Ok((format!("{y}-{m}-01"), last));
    }
    Ok((single.clone(), single))
}

fn is_month(q: &str) -> bool {
    let re = Regex::new(r"^\d{4}-\d{2}$").unwrap();
    re.is_match(q)
}

fn last_day_of_month(y: &str, m: &str) -> Result<String> {
    let y: i32 = y.parse()?;
    let m: u32 = m.parse()?;
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    let first_next = chrono::NaiveDate::from_ymd_opt(ny, nm, 1).ok_or_else(|| anyhow::anyhow!("mes inválido"))?;
    let last = first_next.pred_opt().ok_or_else(|| anyhow::anyhow!("fecha inválida"))?;
    Ok(last.format("%Y-%m-%d").to_string())
}

fn parse_single(q: &str) -> Result<String> {
    let q = q.trim();
    if is_month(q) {
        return Ok(q.to_string());
    }
    let re_iso = Regex::new(r"^(\d{4})-(\d{1,2})-(\d{1,2})$").unwrap();
    if let Some(c) = re_iso.captures(q) {
        return normalize_ymd(&c[1], &c[2], &c[3])
            .ok_or_else(|| anyhow::anyhow!("fecha inválida: {q}"));
    }
    let re_dmy = Regex::new(r"^(\d{1,2})[/-](\d{1,2})[/-](\d{4})$").unwrap();
    if let Some(c) = re_dmy.captures(q) {
        return normalize_dmy(&c[1], &c[2], &c[3])
            .ok_or_else(|| anyhow::anyhow!("fecha inválida: {q}"));
    }
    bail!("no entiendo la fecha '{q}'. Formatos: 2026-09-02 · 02/09/2026 · 2026-09 · 2026-01-01..2026-12-31")
}

/// Búsqueda binaria sobre el índice ordenado.
pub fn query_dates<'a>(idx: &'a DatesIndex, start: &str, end: &str) -> Vec<&'a DateEntry> {
    let lo = idx.dates.partition_point(|e| e.date.as_str() < start);
    let mut out = Vec::new();
    for e in &idx.dates[lo..] {
        if e.date.as_str() > end {
            break;
        }
        if e.date.as_str() >= start {
            out.push(e);
        }
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// búsqueda léxica
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TermHit {
    pub term: String,
    pub hits: usize,
    pub pages: Vec<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
}

/// Normaliza para búsqueda: minúsculas + sin acentos (y mojibake común).
/// Normalización común de texto y de consulta: minúsculas sin acentos y
/// separadores colapsados a un solo espacio.
///
/// Lo de los separadores importa: el texto que se extrae de algunos PDFs
/// separa palabras con tabuladores en vez de espacios. Un texto con tabs no
/// coincide con una consulta con espacios, y una frase que SÍ existe en la
/// página se reporta como ausente — que en una auditoría es una prueba de
/// ausencia inválida. Se normalizan ambos lados por la misma función, así la
/// comparación es simétrica.
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending_sep = false;
    for c in s.to_lowercase().chars() {
        if c.is_whitespace() {
            // Espacios, tabs, saltos de línea: todos equivalentes.
            pending_sep = !out.is_empty();
            continue;
        }
        if pending_sep {
            out.push(' ');
            pending_sep = false;
        }
        let mapped = match c {
            'á' | 'à' | 'ä' | 'â' | 'ã' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' | 'õ' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            _ => c,
        };
        out.push(mapped);
    }
    out
}

pub fn search_blocks(blocks: &[Block], terms: &[String]) -> Vec<TermHit> {
    let folded: Vec<String> = blocks.iter().map(|b| fold(&b.text)).collect();
    terms
        .iter()
        .map(|term| {
            let needle = fold(term);
            let mut pages = BTreeSet::new();
            let mut hits = 0usize;
            let mut snippet = None;
            if !needle.is_empty() {
                for (b, text) in blocks.iter().zip(folded.iter()) {
                    if let Some(pos) = text.find(&needle) {
                        hits += text.match_indices(&needle).count();
                        pages.insert(b.page);
                        if snippet.is_none() {
                            snippet = Some(snippet_around(&b.text, pos, needle.chars().count()));
                        }
                    }
                }
            }
            TermHit {
                term: term.clone(),
                hits,
                pages: pages.into_iter().collect(),
                snippet,
            }
        })
        .collect()
}

fn snippet_around(original: &str, byte_pos: usize, needle_chars: usize) -> String {
    // byte_pos proviene del texto normalizado; aproximamos por caracteres.
    let chars: Vec<char> = original.chars().collect();
    // encontrar la posición aproximada por char usando ratio byte/char
    let approx = (byte_pos * chars.len())
        .checked_div(original.len().max(1))
        .unwrap_or(0);
    let start = approx.saturating_sub(50);
    let end = (approx + needle_chars + 50).min(chars.len());
    let mut s: String = chars[start..end].iter().collect();
    if start > 0 {
        s = format!("…{s}");
    }
    if end < chars.len() {
        s.push('…');
    }
    s.replace('\n', " ")
}

/// Ruta única para un recibo: si ya existe el canónico (misma búsqueda),
/// añade timestamp para no pisar el recibo de una auditoría anterior.
pub fn unique_receipt_path(dir: &Path, qhash: &str) -> PathBuf {
    let base = dir.join(format!("search-{qhash}.json"));
    if !base.exists() {
        return base;
    }
    let ts = chrono::Utc::now().format("%Y%m%d%H%M%S");
    dir.join(format!("search-{qhash}-{ts}.json"))
}

pub fn hash_text(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    format!("{:x}", h.finalize())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchReceipt {
    pub schema: String,
    pub created_at: String,
    pub doc_id: String,
    pub file_name: String,
    pub source_sha256: String,
    pub pages: usize,
    pub blocks: usize,
    pub corpus_sha256: String,
    pub query: Vec<String>,
    pub results: Vec<TermHit>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(id: &str, page: usize, text: &str) -> Block {
        Block {
            id: id.into(),
            node: "n0".into(),
            page,
            kind: "line".into(),
            source: None,
            bbox: Some([0.0, 0.0, 100.0, 10.0]),
            font_size: Some(8.0),
            text: text.into(),
        }
    }

    #[test]
    fn fechas_y_binaria() {
        let blocks = vec![
            block("b1", 1, "Ingreso 02/09/2026 07:00"),
            block("b2", 3, "Egreso 2026-09-18"),
            block("b3", 3, "Control 02/09/2026"),
            block("b4", 5, "Nota 31/12/2026"),
            block("b5", 6, "Teléfono 315 555 0101"), // no debe colarse
        ];
        let idx = build_dates_index("d", "sha", &blocks);
        assert_eq!(idx.dates.len(), 3);
        assert_eq!(idx.dates[0].date, "2026-09-02");
        assert_eq!(idx.dates[0].pages, vec![1, 3]);
        assert_eq!(idx.dates[0].hits, 2);

        let hit = query_dates(&idx, "2026-09-01", "2026-09-30");
        assert_eq!(hit.len(), 2);
        assert_eq!(query_dates(&idx, "2026-12-31", "2026-12-31").len(), 1);
        assert_eq!(query_dates(&idx, "2025-01-01", "2025-12-31").len(), 0);
    }

    #[test]
    fn fechas_largas_en_espanol() {
        let blocks = vec![
            block("b1", 1, "jueves, 23 de julio del 2026 a las 16:31"),
            block("b2", 2, "Control 02-JUL-2026 y otro 3 sep 2026"),
        ];
        let idx = build_dates_index("d", "sha", &blocks);
        let fechas: Vec<&str> = idx.dates.iter().map(|d| d.date.as_str()).collect();
        assert!(fechas.contains(&"2026-07-23"), "{fechas:?}");
        assert!(fechas.contains(&"2026-07-02"), "{fechas:?}");
        assert!(fechas.contains(&"2026-09-03"), "{fechas:?}");
    }

    #[test]
    fn parseo_de_consultas() {
        assert_eq!(
            parse_date_query("02/09/2026").unwrap(),
            ("2026-09-02".to_string(), "2026-09-02".to_string())
        );
        assert_eq!(
            parse_date_query("2026-09").unwrap(),
            ("2026-09-01".to_string(), "2026-09-30".to_string())
        );
        let (a, b) = parse_date_query("2026-01-01..2026-12-31").unwrap();
        assert_eq!((a.as_str(), b.as_str()), ("2026-01-01", "2026-12-31"));
        assert!(parse_date_query("2026-13-01").is_err());
    }

    #[test]
    fn busqueda_sin_acentos() {
        let blocks = vec![
            block("b1", 1, "DIAGNÓSTICO: LINFOMA DE HODGKIN"),
            block("b2", 2, "Se administra brentuximab 127 mg"),
        ];
        let hits = search_blocks(&blocks, &["diagnostico".into(), "BRENTUXIMAB".into(), "karsa".into()]);
        assert_eq!(hits[0].hits, 1);
        assert_eq!(hits[0].pages, vec![1]);
        assert_eq!(hits[1].hits, 1);
        assert_eq!(hits[2].hits, 0);
        assert!(hits[2].pages.is_empty());
    }

    /// El texto de algunos PDFs separa palabras con tabuladores en vez de
    /// espacios. Si la búsqueda no normaliza ese separador, una frase que sí
    /// existe en la página se reporta como AUSENTE. En una auditoría eso es
    /// una prueba de ausencia inválida, que puede sostener una conclusión indebida.
    #[test]
    fn frases_con_tabs_no_producen_ausentes_falsos() {
        let blocks = vec![block(
            "b1",
            7,
            "insuficiencia\trespiratoria\tbilateral\tausente\thoy",
        )];

        let hits = search_blocks(&blocks, &["insuficiencia respiratoria bilateral".into()]);
        assert_eq!(
            hits[0].hits, 1,
            "la frase existe en la página: no debe reportarse como ausente"
        );
        assert_eq!(hits[0].pages, vec![7]);

        // y una ausencia real sigue midiendo 0
        let hits = search_blocks(&blocks, &["pulmon derecho".into()]);
        assert_eq!(hits[0].hits, 0);
        assert!(hits[0].pages.is_empty());
    }
}
