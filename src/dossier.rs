//! `pitk dossier` — consolidación del papel de trabajo.
//!
//! Junta en un solo artefacto (JSON + Markdown) lo que el auditor pega en su
//! papel de trabajo: conciliación de la FEV, cruce facturado vs. soportado,
//! búsquedas de fechas y pruebas de ausencia (recibos).
//!
//! Regla del proyecto: el dossier es **evidencia**, nunca veredictos.

use serde::Serialize;

use crate::cross::CrossReport;
use crate::fev::FevReport;

#[derive(Debug, Clone, Serialize)]
pub struct GroupRow {
    pub name: String,
    pub items: usize,
    pub sum_vt: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declared: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FevBlock {
    pub doc_id: String,
    pub file_name: String,
    pub mode: String,
    pub items: usize,
    pub sum_vt: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declared_total: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declared_lines: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<f64>,
    pub ok: bool,
    pub groups: Vec<GroupRow>,
    pub missing: Vec<String>,
    pub duplicates: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DocRow {
    pub file_name: String,
    pub doc_id: String,
    pub pages: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ItemRow {
    pub linea: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    pub code1: String,
    pub desc: String,
    pub qty: f64,
    pub vt: f64,
    /// soportado | sin_soporte
    pub status: String,
    /// "soporte.pdf p.12,15 (término, …)" o "—"
    pub support: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CrossBlock {
    pub items: usize,
    pub supported: usize,
    pub without_support: usize,
    pub coverage_pct: f64,
    pub docs: Vec<DocRow>,
    pub rows: Vec<ItemRow>,
    pub without_support_rows: Vec<ItemRow>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DatesRow {
    pub query: String,
    pub start: String,
    pub end: String,
    pub days: usize,
    pub hits: usize,
    pub pages: Vec<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReceiptRow {
    pub file: String,
    pub created_at: String,
    pub query: Vec<String>,
    pub hits: usize,
    pub corpus_sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Dossier {
    pub schema: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    pub fev: FevBlock,
    pub cross: Option<CrossBlock>,
    pub dates: Vec<DatesRow>,
    pub receipts: Vec<ReceiptRow>,
    pub warnings: Vec<String>,
}

pub fn build(
    account: Option<String>,
    created_at: String,
    fev: &FevReport,
    cross: Option<&CrossReport>,
    dates: Vec<DatesRow>,
    receipts: Vec<ReceiptRow>,
) -> Dossier {
    let mut warnings: Vec<String> = Vec::new();
    warnings.extend(fev.warnings.iter().cloned());
    if cross.is_none() {
        warnings.push(
            "sin cruce: pasa --docs HEV.pdf HAM.pdf … o ejecuta `pitk cross` primero".to_string(),
        );
    }
    if let Some(c) = cross {
        warnings.extend(c.warnings.iter().cloned());
        if c.summary.without_support > 0 {
            warnings.push(format!(
                "{} ítem(s) sin soporte documental en los PDFs provistos (candidatos a revisión)",
                c.summary.without_support
            ));
        }
    }
    if !fev.reconciliation.ok {
        warnings.push(
            "la FEV no reconcilia: revisa `pitk fev` antes de citar cifras".to_string(),
        );
    }

    let fev_block = FevBlock {
        doc_id: fev.doc_id.clone(),
        file_name: fev.file_name.clone(),
        mode: fev.mode.clone(),
        items: fev.items.len(),
        sum_vt: fev.reconciliation.sum_vt,
        declared_total: fev.reconciliation.declared_total,
        declared_lines: fev.reconciliation.declared_lines,
        diff: fev.reconciliation.diff,
        ok: fev.reconciliation.ok,
        groups: fev
            .groups
            .iter()
            .map(|g| GroupRow {
                name: g.name.clone(),
                items: g.items,
                sum_vt: g.sum_vt,
                declared: g.declared,
                diff: g.diff,
            })
            .collect(),
        missing: fev.reconciliation.missing.clone(),
        duplicates: fev.reconciliation.duplicates.clone(),
    };

    let cross_block = cross.map(|c| {
        let rows: Vec<ItemRow> = c
            .items
            .iter()
            .map(|it| ItemRow {
                linea: it.linea,
                group: it.group.clone(),
                code1: it.code1.clone(),
                desc: it.desc.clone(),
                qty: it.qty,
                vt: it.vt,
                status: it.status.clone(),
                support: format_support(&it.support),
            })
            .collect();
        let without = rows
            .iter()
            .filter(|r| r.status == "sin_soporte")
            .cloned()
            .collect();
        CrossBlock {
            items: c.summary.items,
            supported: c.summary.supported,
            without_support: c.summary.without_support,
            coverage_pct: c.summary.coverage_pct,
            docs: c
                .docs
                .iter()
                .map(|d| DocRow {
                    file_name: d.file_name.clone(),
                    doc_id: d.doc_id.clone(),
                    pages: d.pages,
                })
                .collect(),
            rows,
            without_support_rows: without,
        }
    });

    Dossier {
        schema: "pitk/dossier/1".to_string(),
        created_at,
        account,
        fev: fev_block,
        cross: cross_block,
        dates,
        receipts,
        warnings,
    }
}

fn format_support(hits: &[crate::cross::DocHit]) -> String {
    if hits.is_empty() {
        return "—".to_string();
    }
    let mut parts: Vec<String> = Vec::new();
    for h in hits {
        let pages = h
            .pages
            .iter()
            .take(4)
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let terms = h.terms.join(", ");
        let extra = if h.pages.len() > 4 {
            format!("+{}", h.pages.len() - 4)
        } else {
            String::new()
        };
        parts.push(format!("{} p.{}{} ({})", h.file_name, pages, extra, terms));
    }
    parts.join(" · ")
}

fn money(v: f64) -> String {
    // 1234567 → 1.234.567 (separador de miles)
    let neg = v < 0.0;
    let s = format!("{:.2}", v.abs());
    let (int_part, dec) = s.split_once('.').unwrap_or((s.as_str(), "00"));
    let mut out = String::new();
    for (i, ch) in int_part.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push('.');
        }
        out.push(ch);
    }
    let int: String = out.chars().rev().collect();
    format!("{}{},{}", if neg { "-" } else { "" }, int, dec)
}

fn md_escape(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

pub fn render_md(d: &Dossier) -> String {
    use std::fmt::Write as _;
    let mut o = String::new();

    let _ = writeln!(o, "# Dossier de evidencia — {}", md_escape(&d.fev.file_name));
    let _ = writeln!(o);
    let _ = writeln!(o, "- **Cuenta:** {}", d.account.as_deref().unwrap_or("(sin anclar)"));
    let _ = writeln!(o, "- **Generado:** {}", d.created_at);
    let _ = writeln!(o, "- **FEV doc_id:** `{}` · modo `{}`", d.fev.doc_id, d.fev.mode);
    let _ = writeln!(
        o,
        "- **Aviso:** este dossier es evidencia reproducible; las decisiones finales son del auditor."
    );
    let _ = writeln!(o);

    // 1. Conciliación
    let _ = writeln!(o, "## 1. Conciliación de la FEV");
    let _ = writeln!(o);
    let _ = writeln!(o, "| Ítems | Suma VT | Total declarado | Diferencia | Estado |");
    let _ = writeln!(o, "|---:|---:|---:|---:|---|");
    let _ = writeln!(
        o,
        "| {} | ${} | {} | {} | {} |",
        d.fev.items,
        money(d.fev.sum_vt),
        d.fev
            .declared_total
            .map(|t| format!("${}", money(t)))
            .unwrap_or_else(|| "—".into()),
        d.fev
            .diff
            .map(|x| format!("{}{}", if x > 0.0 { "+" } else { "" }, money(x)))
            .unwrap_or_else(|| "—".into()),
        if d.fev.ok { "✔ reconcilia" } else { "✗ revisar" }
    );
    if !d.fev.groups.is_empty() {
        let _ = writeln!(o);
        let _ = writeln!(o, "| Grupo | Ítems | Suma VT | Declarado |");
        let _ = writeln!(o, "|---|---:|---:|---:|");
        for g in &d.fev.groups {
            let _ = writeln!(
                o,
                "| {} | {} | ${} | {} |",
                md_escape(&g.name),
                g.items,
                money(g.sum_vt),
                g.declared.map(|t| format!("${}", money(t))).unwrap_or_else(|| "—".into())
            );
        }
    }
    if !d.fev.missing.is_empty() {
        let _ = writeln!(o);
        let _ = writeln!(o, "**Líneas faltantes:**");
        for m in &d.fev.missing {
            let _ = writeln!(o, "- {m}");
        }
    }
    if !d.fev.duplicates.is_empty() {
        let _ = writeln!(o);
        let _ = writeln!(o, "**Líneas duplicadas:**");
        for m in &d.fev.duplicates {
            let _ = writeln!(o, "- {m}");
        }
    }
    let _ = writeln!(o);

    // 2. Cruce
    let _ = writeln!(o, "## 2. Cruce facturado vs. soportado");
    let _ = writeln!(o);
    match &d.cross {
        None => {
            let _ = writeln!(o, "_Sin cruce: no se pasaron documentos de soporte._");
        }
        Some(c) => {
            let _ = writeln!(
                o,
                "- Ítems: **{}** · con soporte: **{}** · sin soporte: **{}** · cobertura: **{:.1}%**",
                c.items, c.supported, c.without_support, c.coverage_pct
            );
            let docs = c
                .docs
                .iter()
                .map(|x| format!("{} ({} págs)", x.file_name, x.pages))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(o, "- Documentos cruzados: {docs}");
            let _ = writeln!(o);
            let _ = writeln!(o, "| # | Código | Descripción | Cant | VT | Soporte |");
            let _ = writeln!(o, "|---:|---|---|---:|---:|---|");
            for r in &c.rows {
                let _ = writeln!(
                    o,
                    "| {} | {} | {} | {} | ${} | {} |",
                    r.linea,
                    md_escape(&r.code1),
                    md_escape(&r.desc),
                    r.qty,
                    money(r.vt),
                    md_escape(&r.support)
                );
            }
            if !c.without_support_rows.is_empty() {
                let _ = writeln!(o);
                let _ = writeln!(
                    o,
                    "### Ítems sin soporte documental (candidatos, NO veredictos)"
                );
                for r in &c.without_support_rows {
                    let _ = writeln!(
                        o,
                        "- **{}** `{}` {} — {} × ${} = ${}",
                        r.linea,
                        r.code1,
                        r.desc,
                        r.qty,
                        money(if r.qty > 0.0 { r.vt / r.qty } else { 0.0 }),
                        money(r.vt)
                    );
                }
            }
        }
    }
    let _ = writeln!(o);

    // 3. Fechas
    let _ = writeln!(o, "## 3. Búsquedas de fechas");
    let _ = writeln!(o);
    if d.dates.is_empty() {
        let _ = writeln!(o, "_Sin consultas de fecha._");
    } else {
        let _ = writeln!(o, "| Consulta | Rango | Días | Ocurrencias | Páginas |");
        let _ = writeln!(o, "|---|---|---:|---:|---|");
        for r in &d.dates {
            let pages = r
                .pages
                .iter()
                .take(15)
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join(",");
            let extra = if r.pages.len() > 15 {
                format!(" +{}", r.pages.len() - 15)
            } else {
                String::new()
            };
            let _ = writeln!(
                o,
                "| `{}` | {} .. {} | {} | {} | {}{} |",
                r.query, r.start, r.end, r.days, r.hits, pages, extra
            );
        }
    }
    let _ = writeln!(o);

    // 4. Pruebas de ausencia
    let _ = writeln!(o, "## 4. Pruebas de ausencia (recibos)");
    let _ = writeln!(o);
    if d.receipts.is_empty() {
        let _ = writeln!(
            o,
            "_Sin recibos: usa `pitk search <pdf> <términos> --receipt` para dejar constancia._"
        );
    } else {
        let _ = writeln!(o, "| Recibo | Términos | Ocurrencias | Corpus sha256 |");
        let _ = writeln!(o, "|---|---|---:|---|");
        for r in &d.receipts {
            let _ = writeln!(
                o,
                "| {} | {} | {} | `{}` |",
                r.file,
                r.query.join(", "),
                r.hits,
                &r.corpus_sha256[..r.corpus_sha256.len().min(12)]
            );
        }
        let _ = writeln!(o);
        let _ = writeln!(
            o,
            "> Un recibo prueba que el término se buscó sobre un corpus con hash; úsalo al sustentar ausencias."
        );
    }
    let _ = writeln!(o);

    if !d.warnings.is_empty() {
        let _ = writeln!(o, "## Advertencias");
        let _ = writeln!(o);
        for w in &d.warnings {
            let _ = writeln!(o, "- {w}");
        }
        let _ = writeln!(o);
    }

    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cross::{CrossItem, CrossReport, DocHit, DocMeta, Summary};
    use crate::fev::{FevGroup, FevItem, FevReport, Reconciliation};

    fn item(linea: usize, code: &str, desc: &str, qty: f64, vt: f64) -> FevItem {
        FevItem {
            linea,
            group: Some("MEDICAMENTOS".into()),
            code1: code.into(),
            code2: None,
            desc: desc.into(),
            autorizacion: None,
            qty,
            vu: vt / qty,
            vt,
            page: 1,
            source: "line".into(),
        }
    }

    fn fev() -> FevReport {
        FevReport {
            doc_id: "abc123def456".into(),
            file_name: "FEV.pdf".into(),
            tool_version: "test".into(),
            mode: "inline".into(),
            items: vec![
                item(1, "1000", "SERVICIO ALFA", 1.0, 100_000.0),
                item(2, "2000", "SERVICIO BETA", 3.0, 200_000.0),
            ],
            groups: vec![FevGroup {
                name: "MEDICAMENTOS".into(),
                items: 2,
                sum_vt: 300_000.0,
                declared: Some(300_000.0),
                diff: Some(0.0),
            }],
            reconciliation: Reconciliation {
                items: 2,
                declared_lines: Some(2),
                declared_total: Some(300_000.0),
                sum_vt: 300_000.0,
                diff: Some(0.0),
                arithmetic_ok: true,
                ok: true,
                duplicates: vec![],
                missing: vec![],
            },
            warnings: vec![],
        }
    }

    fn cross() -> CrossReport {
        CrossReport {
            schema: "pitk/cross/1".into(),
            fev_doc_id: "abc123def456".into(),
            fev_file_name: "FEV.pdf".into(),
            docs: vec![DocMeta {
                doc_id: "ham".into(),
                file_name: "HAM.pdf".into(),
                pages: 4,
                blocks: 10,
            }],
            items: vec![
                CrossItem {
                    linea: 1,
                    group: Some("MEDICAMENTOS".into()),
                    code1: "1000".into(),
                    desc: "SERVICIO ALFA".into(),
                    qty: 1.0,
                    vt: 100_000.0,
                    support: vec![DocHit {
                        doc_id: "ham".into(),
                        file_name: "HAM.pdf".into(),
                        pages: vec![4],
                        hits: 3,
                        terms: vec!["soporte".into()],
                    }],
                    status: "soportado".into(),
                },
                CrossItem {
                    linea: 2,
                    group: Some("MEDICAMENTOS".into()),
                    code1: "2000".into(),
                    desc: "SERVICIO BETA".into(),
                    qty: 3.0,
                    vt: 200_000.0,
                    support: vec![],
                    status: "sin_soporte".into(),
                },
            ],
            summary: Summary {
                items: 2,
                supported: 1,
                without_support: 1,
                coverage_pct: 50.0,
                by_doc: Default::default(),
            },
            warnings: vec![],
        }
    }

    #[test]
    fn dossier_md_incluye_conciliacion_cruce_fechas_y_recibos() {
        let d = build(
            Some("CUENTA-DEMO".into()),
            "2026-10-19T00:00:00Z".into(),
            &fev(),
            Some(&cross()),
            vec![DatesRow {
                query: "2026-09".into(),
                start: "2026-09-01".into(),
                end: "2026-09-30".into(),
                days: 7,
                hits: 12,
                pages: vec![1, 2, 3],
            }],
            vec![ReceiptRow {
                file: "2026-10-19T00-00-00_abc123.json".into(),
                created_at: "2026-10-19T00:00:00Z".into(),
                query: vec!["término-inexistente".into()],
                hits: 0,
                corpus_sha256: "f".repeat(64),
            }],
        );
        let md = render_md(&d);
        assert!(md.contains("## 1. Conciliación de la FEV"), "{md}");
        assert!(md.contains("$300.000,00"), "suma formateada: {md}");
        assert!(md.contains("✔ reconcilia"), "{md}");
        assert!(md.contains("## 2. Cruce facturado vs. soportado"), "{md}");
        assert!(md.contains("| 1 | 1000 |"), "{md}");
        assert!(md.contains("soportado"), "{md}");
        assert!(md.contains("Ítems sin soporte documental"), "{md}");
        assert!(md.contains("`2000`"), "{md}");
        assert!(md.contains("50.0%"), "{md}");
        assert!(md.contains("| `2026-09` |"), "{md}");
        assert!(md.contains("2026-09-01 .. 2026-09-30"), "{md}");
        assert!(md.contains("2026-10-19T00-00-00_abc123.json"), "{md}");
        assert!(md.contains("1 ítem(s) sin soporte"), "{md}");
    }

    #[test]
    fn dossier_sin_cruce_avisa_y_no_inventa_soporte() {
        let d = build(None, "2026-10-19T00:00:00Z".into(), &fev(), None, vec![], vec![]);
        let md = render_md(&d);
        assert!(md.contains("_Sin cruce"), "{md}");
        assert!(md.contains("sin cruce: pasa --docs"), "{md}");
        assert!(!md.contains("Ítems sin soporte documental"), "{md}");
    }

    #[test]
    fn dossier_marca_fev_que_no_reconcilia() {
        let mut f = fev();
        f.reconciliation.ok = false;
        f.reconciliation.diff = Some(-1000.0);
        let d = build(None, "2026-10-19T00:00:00Z".into(), &f, None, vec![], vec![]);
        let md = render_md(&d);
        assert!(md.contains("✗ revisar"), "{md}");
        assert!(md.contains("la FEV no reconcilia"), "{md}");
    }
}
