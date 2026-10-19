//! `pitk flags` — candidatos de revisión sobre los ítems de un documento tabular.
//!
//! El motor NO emite veredictos: produce **señales** reproducibles para que el
//! auditor decida qué mirar. Cada señal lleva su evidencia (línea, página,
//! valores) y un código estable para citarla en el papel de trabajo.
//!
//! Funciona con cualquier documento tabular de ítems/precios (factura, cuenta
//! de cobro, estado de cuenta): no asume un dominio concreto.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::cross::CrossReport;
use crate::fev::FevReport;

/// Tolerancia aritmética: 0.5 unidades o 0.05% del valor, lo que sea mayor.
pub fn tol(vt: f64) -> f64 {
    0.5f64.max(vt.abs() * 0.0005)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Flag {
    pub code: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlaggedItem {
    pub linea: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    pub code1: String,
    pub desc: String,
    pub qty: f64,
    pub vu: f64,
    pub vt: f64,
    pub page: usize,
    pub flags: Vec<Flag>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlagsReport {
    pub schema: String,
    pub doc_id: String,
    pub file_name: String,
    pub items: usize,
    pub flagged_items: usize,
    pub by_code: BTreeMap<String, usize>,
    pub candidates: Vec<FlaggedItem>,
    pub warnings: Vec<String>,
}

/// Corre todas las señales disponibles. `cross` es opcional: sin él, las señales
/// que dependen de soporte documental simplemente no se emiten.
pub fn run(fev: &FevReport, cross: Option<&CrossReport>) -> FlagsReport {
    let mut warnings: Vec<String> = Vec::new();

    // ¿El documento declara la columna de autorización? Si no, no marcamos
    // ausencias de autorización (sería ruido sobre un campo que no existe).
    let auth_column = fev.items.iter().any(|i| i.autorizacion.is_some());
    if !auth_column {
        warnings.push(
            "el documento no declara autorizaciones: no se emiten señales de autorización"
                .to_string(),
        );
    }

    // El cruce se alinea por posición cuando ambos tienen el mismo largo;
    // si no, por (línea, grupo) — los números de línea se repiten por grupo.
    let support: Option<Vec<bool>> = cross.map(|c| {
        if c.items.len() == fev.items.len() {
            c.items.iter().map(|i| i.status == "sin_soporte").collect()
        } else {
            fev.items
                .iter()
                .map(|f| {
                    c.items
                        .iter()
                        .find(|i| i.linea == f.linea && i.group == f.group)
                        .map(|i| i.status == "sin_soporte")
                        .unwrap_or(false)
                })
                .collect()
        }
    });
    if cross.is_none() {
        warnings.push("sin cruce: no se emiten señales de soporte (--cross)".to_string());
    }

    let mut by_code: BTreeMap<String, usize> = BTreeMap::new();
    let mut candidates: Vec<FlaggedItem> = Vec::new();

    for (idx, item) in fev.items.iter().enumerate() {
        let mut flags: Vec<Flag> = Vec::new();

        // 1) aritmética del ítem: cantidad × unitario = total
        let calc = item.qty * item.vu;
        if (calc - item.vt).abs() > tol(item.vt) {
            flags.push(Flag {
                code: "aritmetica".into(),
                detail: format!(
                    "cantidad × unitario = {:.2} pero el total dice {:.2}",
                    calc, item.vt
                ),
            });
        }

        // 2) cantidades o valores no utilizables
        if item.qty <= 0.0 {
            flags.push(Flag {
                code: "cantidad_no_positiva".into(),
                detail: format!("cantidad = {:.4}", item.qty),
            });
        }
        if item.vt <= 0.0 && item.qty > 0.0 {
            flags.push(Flag {
                code: "valor_cero".into(),
                detail: "total ≤ 0 con cantidad positiva".into(),
            });
        }

        // 3) código ausente o ilegible
        if item.code1.trim().is_empty()
            || !item.code1.chars().any(|c| c.is_ascii_alphanumeric())
        {
            flags.push(Flag {
                code: "sin_codigo".into(),
                detail: "no se pudo extraer un código de ítem".into(),
            });
        }

        // 4) autorización declarada en el documento pero ausente en el ítem
        if auth_column && item.autorizacion.is_none() {
            flags.push(Flag {
                code: "autorizacion_ausente".into(),
                detail: "la columna existe pero este ítem no trae valor".into(),
            });
        }

        // 5) sin soporte documental (solo si hay cruce)
        if support.as_ref().map(|s| s[idx]).unwrap_or(false) {
            flags.push(Flag {
                code: "sin_soporte".into(),
                detail: "ningún documento cruzado menciona el ítem".into(),
            });
        }

        if flags.is_empty() {
            continue;
        }
        for f in &flags {
            *by_code.entry(f.code.clone()).or_default() += 1;
        }
        candidates.push(FlaggedItem {
            linea: item.linea,
            group: item.group.clone(),
            code1: item.code1.clone(),
            desc: item.desc.clone(),
            qty: item.qty,
            vu: item.vu,
            vt: item.vt,
            page: item.page,
            flags,
        });
    }

    FlagsReport {
        schema: "pitk/flags/1".to_string(),
        doc_id: fev.doc_id.clone(),
        file_name: fev.file_name.clone(),
        items: fev.items.len(),
        flagged_items: candidates.len(),
        by_code,
        candidates,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cross::{CrossItem, CrossReport, Summary};
    use crate::fev::{FevGroup, FevItem, Reconciliation};

    fn item(linea: usize, code: &str, qty: f64, vu: f64, vt: f64) -> FevItem {
        FevItem {
            linea,
            group: None,
            code1: code.into(),
            code2: None,
            desc: format!("SERVICIO {linea}"),
            autorizacion: None,
            qty,
            vu,
            vt,
            page: 1,
            source: "line".into(),
        }
    }

    fn fev(items: Vec<FevItem>) -> FevReport {
        let sum: f64 = items.iter().map(|i| i.vt).sum();
        FevReport {
            doc_id: "d0d0d0d0d0d0".into(),
            file_name: "factura.pdf".into(),
            tool_version: "test".into(),
            mode: "inline".into(),
            items: items.clone(),
            groups: vec![FevGroup { name: "(sin grupo)".into(), items: items.len(), sum_vt: sum, declared: None, diff: None }],
            reconciliation: Reconciliation {
                items: items.len(),
                declared_lines: None,
                declared_total: Some(sum),
                sum_vt: sum,
                diff: Some(0.0),
                arithmetic_ok: true,
                ok: true,
                duplicates: vec![],
                missing: vec![],
            },
            warnings: vec![],
        }
    }

    #[test]
    fn marca_aritmetica_rota_y_deja_pasar_la_correcta() {
        let ok = item(1, "1000", 2.0, 50.0, 100.0);
        let bad = item(2, "2000", 2.0, 100.0, 150.0);
        let r = run(&fev(vec![ok, bad]), None);
        assert_eq!(r.flagged_items, 1);
        assert_eq!(r.by_code.get("aritmetica"), Some(&1));
        assert_eq!(r.candidates[0].linea, 2);
        assert!(r.candidates[0].flags[0].detail.contains("200"));
    }

    #[test]
    fn autorizacion_solo_si_el_documento_la_declara() {
        let mut a = item(1, "1000", 1.0, 10.0, 10.0);
        let mut b = item(2, "2000", 1.0, 10.0, 10.0);
        // sin columna declarada: no hay señales y avisa
        let r = run(&fev(vec![a.clone(), b.clone()]), None);
        assert_eq!(r.flagged_items, 0);
        assert!(r.warnings.iter().any(|w| w.contains("autorizaciones")));

        // con columna declarada: el ítem que no trae valor queda marcado
        a.autorizacion = Some("AU-1".into());
        b.autorizacion = None;
        let r = run(&fev(vec![a, b]), None);
        assert_eq!(r.by_code.get("autorizacion_ausente"), Some(&1));
        assert_eq!(r.candidates[0].linea, 2);
    }

    #[test]
    fn sin_soporte_se_alinea_por_posicion_aunque_la_linea_se_repita() {
        // Documento real: la numeración de línea se repite por grupo (1,2,1,2,3...)
        let items = vec![
            item(1, "A", 1.0, 10.0, 10.0),
            item(2, "B", 1.0, 10.0, 10.0),
        ];
        let mut items2 = items.clone();
        items2[0].group = Some("G2".into());
        items2[1].group = Some("G2".into());
        let report = fev(items2);

        let cross = CrossReport {
            schema: "pitk/cross/1".into(),
            fev_doc_id: report.doc_id.clone(),
            fev_file_name: report.file_name.clone(),
            docs: vec![],
            items: vec![
                CrossItem { linea: 1, group: None, code1: "A".into(), desc: "A".into(), qty: 1.0, vt: 10.0, support: vec![], status: "soportado".into() },
                CrossItem { linea: 2, group: Some("G2".into()), code1: "B".into(), desc: "B".into(), qty: 1.0, vt: 10.0, support: vec![], status: "sin_soporte".into() },
            ],
            summary: Summary { items: 2, supported: 1, without_support: 1, coverage_pct: 50.0, by_doc: Default::default() },
            warnings: vec![],
        };
        let r = run(&report, Some(&cross));
        assert_eq!(r.by_code.get("sin_soporte"), Some(&1));
        assert_eq!(r.candidates[0].linea, 2);
        assert_eq!(r.candidates[0].group.as_deref(), Some("G2"));
    }

    #[test]
    fn sin_codigo_y_valores_inutilizables() {
        let mut a = item(1, "", 0.0, 10.0, 0.0);
        a.code1 = "---".into();
        let r = run(&fev(vec![a]), None);
        assert!(r.by_code.contains_key("sin_codigo"));
        assert!(r.by_code.contains_key("cantidad_no_positiva"));
    }
}
