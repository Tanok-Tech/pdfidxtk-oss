//! `pitk cross` — cruce de los ítems de un documento tabular (p. ej. una
//! factura) contra los documentos de soporte del caso (anexos, actas,
//! contratos, soportes…).
//!
//! Para cada ítem del documento busca sus términos distintivos (y su código) en
//! los bloques de cada documento y reporta: ¿tiene soporte?, ¿en qué documento
//! y páginas?, ¿con qué términos? Es el insumo directo del "cruce ítems vs.
//! soporte" del papel de trabajo.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use crate::fev::FevItem;
use crate::model::Block;
use crate::query::{fold, search_blocks};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocMeta {
    pub doc_id: String,
    pub file_name: String,
    pub pages: usize,
    pub blocks: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocHit {
    pub doc_id: String,
    pub file_name: String,
    pub pages: Vec<usize>,
    pub hits: usize,
    pub terms: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossItem {
    pub linea: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    pub code1: String,
    pub desc: String,
    pub qty: f64,
    pub vt: f64,
    pub support: Vec<DocHit>,
    /// soportado | sin_soporte
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Summary {
    pub items: usize,
    pub supported: usize,
    pub without_support: usize,
    pub coverage_pct: f64,
    /// doc_id → ítems que tienen al menos una coincidencia en ese documento
    pub by_doc: std::collections::BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossReport {
    pub schema: String,
    pub fev_doc_id: String,
    pub fev_file_name: String,
    pub docs: Vec<DocMeta>,
    pub items: Vec<CrossItem>,
    pub summary: Summary,
    pub warnings: Vec<String>,
}

pub const STOPWORDS: &[&str] = &[
    "unidad", "unidades", "servicio", "servicios", "procedimiento", "procedimientos",
    "medicamento", "medicamentos", "insumo", "insumos", "dispositivo", "dispositivos",
    "equipo", "equipos", "solucion", "soluciones", "inyectable", "inyectables",
    "inyeccion", "inyecciones", "miligramo", "miligramos", "gramo", "gramos",
    "aplica", "autorizacion", "autorizaciones",
    "mililitro", "mililitros", "litro", "litros", "centimetro", "milimetros",
    "persona", "operador", "operadora", "quirurgico", "quirurgica", "quirurgicos",
    "honorarios", "derecho", "derechos", "sala", "materiales", "material",
    "administracion", "continua", "intravenosa", "intravenoso", "parenteral",
    "paquete", "paquetes", "global", "otras", "otros", "cada", "caja", "frasco",
    "ampolla", "vial", "bolsa", "polvo", "liofilizado", "concentrado",
];

/// Términos distintivos del ítem: tokens largos no genéricos + código
/// identificador del ítem.
pub fn key_terms(item: &FevItem, synonyms: &[(String, String)]) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();

    // el código es un identificador fuerte (se busca como token)
    let code = item.code1.trim();
    if code.len() >= 4 && code.chars().any(|c| c.is_ascii_digit()) {
        terms.push(code.to_string());
    }

    let mut seen = BTreeSet::new();
    let folded = fold(&item.desc);
    let mut words: Vec<String> = folded
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 5)
        .filter(|w| !STOPWORDS.contains(w))
        .filter(|w| !w.chars().all(|c| c.is_ascii_digit()))
        .map(|w| w.to_string())
        .collect();
    // los más largos primero: más distintivos
    words.sort_by_key(|w| std::cmp::Reverse(w.chars().count()));
    let mut words_added = 0usize;
    for w in words {
        if seen.insert(w.clone()) {
            terms.push(w);
            words_added += 1;
        }
        if words_added >= 4 {
            break;
        }
    }

    expand_synonyms(&mut terms, &folded, synonyms);
    terms
}

/// Sinónimos opcionales del workspace (`.docidx/synonyms.json`). El motor no
/// trae vocabulario de ningún dominio: cada caso aporta sus equivalencias.
/// Acepta `{"término": "equivalente"}` o `[["término", "equivalente"], …]`.
pub fn load_synonyms(store_dir: &Path) -> Vec<(String, String)> {
    let path = store_dir.join("synonyms.json");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    match v {
        serde_json::Value::Object(map) => {
            for (k, val) in map {
                if let Some(s) = val.as_str() {
                    out.push((k.to_lowercase(), s.to_lowercase()));
                }
            }
        }
        serde_json::Value::Array(items) => {
            for it in items {
                if let Some(pair) = it.as_array() {
                    if let (Some(a), Some(b)) = (pair.first(), pair.get(1)) {
                        if let (Some(a), Some(b)) = (a.as_str(), b.as_str()) {
                            out.push((a.to_lowercase(), b.to_lowercase()));
                        }
                    }
                }
            }
        }
        _ => {}
    }
    out
}

/// Expansión de sinónimos configurados por el caso: el documento tabular y el
/// documento de soporte no siempre usan las mismas palabras para el mismo objeto.
fn expand_synonyms(terms: &mut Vec<String>, folded_desc: &str, synonyms: &[(String, String)]) {
    for (needle, add) in synonyms {
        if folded_desc.contains(needle.as_str()) && !terms.iter().any(|t| t == add) {
            terms.push(add.clone());
        }
    }
}

/// Índice de un documento: palabras normalizadas → (ocurrencias, páginas).
/// Evita escanear 80k bloques por cada ítem en corpus largos.
struct CorpusIndex<'a> {
    blocks: &'a [Block],
    folded: Vec<String>,
    words: HashMap<String, (usize, BTreeSet<usize>)>,
    cache: HashMap<String, (usize, Vec<usize>)>,
}

impl<'a> CorpusIndex<'a> {
    fn new(blocks: &'a [Block]) -> Self {
        let folded: Vec<String> = blocks.iter().map(|b| fold(&b.text)).collect();
        let mut words: HashMap<String, (usize, BTreeSet<usize>)> = HashMap::new();
        for (b, text) in blocks.iter().zip(folded.iter()) {
            for w in text.split(|c: char| !c.is_alphanumeric()) {
                if w.chars().count() < 2 {
                    continue;
                }
                let e = words.entry(w.to_string()).or_insert_with(|| (0, BTreeSet::new()));
                e.0 += 1;
                e.1.insert(b.page);
            }
        }
        Self {
            blocks,
            folded,
            words,
            cache: HashMap::new(),
        }
    }

    fn lookup(&mut self, term: &str) -> (usize, Vec<usize>) {
        let key = term.to_string();
        if let Some(v) = self.cache.get(&key) {
            return v.clone();
        }
        let (hits, pages) = if term.chars().all(|c| c.is_alphanumeric()) {
            match self.words.get(term) {
                Some((h, p)) => (*h, p.iter().copied().collect()),
                None => (0, Vec::new()),
            }
        } else {
            // términos con guiones u otros signos (códigos): búsqueda literal
            let mut pages = BTreeSet::new();
            let mut hits = 0usize;
            for (b, text) in self.blocks.iter().zip(self.folded.iter()) {
                let n = text.matches(term).count();
                if n > 0 {
                    hits += n;
                    pages.insert(b.page);
                }
            }
            (hits, pages.into_iter().collect())
        };
        self.cache.insert(key, (hits, pages.clone()));
        (hits, pages)
    }
}

pub fn run(
    fev_doc_id: &str,
    fev_file_name: &str,
    items: &[FevItem],
    docs: &[(DocMeta, Vec<Block>)],
    synonyms: &[(String, String)],
) -> CrossReport {
    let mut out_items = Vec::with_capacity(items.len());
    let mut by_doc: std::collections::BTreeMap<String, usize> =
        docs.iter().map(|(m, _)| (m.doc_id.clone(), 0)).collect();
    let mut supported = 0usize;
    let mut warnings: Vec<String> = Vec::new();

    let mut indexes: Vec<(&DocMeta, CorpusIndex)> = docs
        .iter()
        .map(|(m, blocks)| (m, CorpusIndex::new(blocks)))
        .collect();

    for item in items {
        let terms = key_terms(item, synonyms);
        let mut support: Vec<DocHit> = Vec::new();

        for (meta, index) in indexes.iter_mut() {
            let mut pages = BTreeSet::new();
            let mut total_hits = 0usize;
            let mut matched: Vec<String> = Vec::new();
            for term in &terms {
                let (hits, term_pages) = index.lookup(term);
                if hits > 0 {
                    total_hits += hits;
                    pages.extend(term_pages);
                    matched.push(term.clone());
                }
            }
            if total_hits > 0 {
                *by_doc.entry(meta.doc_id.clone()).or_insert(0) += 1;
                support.push(DocHit {
                    doc_id: meta.doc_id.clone(),
                    file_name: meta.file_name.clone(),
                    pages: pages.into_iter().collect(),
                    hits: total_hits,
                    terms: matched,
                });
            }
        }

        let status = if support.is_empty() {
            "sin_soporte".to_string()
        } else {
            supported += 1;
            "soportado".to_string()
        };

        if terms.is_empty() {
            warnings.push(format!(
                "ítem {} sin términos distintivos utilizables (descripción vacía o genérica)",
                item.linea
            ));
        }

        out_items.push(CrossItem {
            linea: item.linea,
            group: item.group.clone(),
            code1: item.code1.clone(),
            desc: item.desc.clone(),
            qty: item.qty,
            vt: item.vt,
            support,
            status,
        });
    }

    let total = out_items.len();
    let coverage_pct = if total == 0 {
        0.0
    } else {
        (supported as f64 / total as f64) * 100.0
    };

    CrossReport {
        schema: "pitk/cross/1".to_string(),
        fev_doc_id: fev_doc_id.to_string(),
        fev_file_name: fev_file_name.to_string(),
        docs: docs.iter().map(|(m, _)| m.clone()).collect(),
        items: out_items,
        summary: Summary {
            items: total,
            supported,
            without_support: total - supported,
            coverage_pct: (coverage_pct * 10.0).round() / 10.0,
            by_doc,
        },
        warnings,
    }
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

    fn item(linea: usize, code: &str, desc: &str) -> FevItem {
        FevItem {
            linea,
            group: None,
            code1: code.into(),
            code2: None,
            desc: desc.into(),
            autorizacion: None,
            qty: 1.0,
            vu: 100.0,
            vt: 100.0,
            page: 1,
            source: "test".into(),
        }
    }

    #[test]
    fn terminos_distintivos() {
        let it = item(
            1,
            "20058697-1",
            "AGLUTINANTE EJEMPLO X 50 UNIDAD(ES) EN POLVO LIOFILIZADO PARA INYECCION",
        );
        let t = key_terms(&it, &[]);
        assert!(t.contains(&"20058697-1".to_string()));
        assert!(t.iter().any(|x| x == "aglutinante"));
        assert!(t.iter().any(|x| x == "ejemplo"));
        // stopwords fuera
        assert!(!t.iter().any(|x| x == "polvo" || x == "inyeccion"));
    }

    #[test]
    fn cruce_soporta_y_marca_ausencia() {
        let items = vec![
            item(1, "1000", "AGLUTINANTE EJEMPLO"),
            item(2, "999999", "INEXISTENTE VIRTUAL"),
        ];
        let docs = vec![
            (
                DocMeta {
                    doc_id: "doc1".into(),
                    file_name: "soporte.pdf".into(),
                    pages: 4,
                    blocks: 10,
                },
                vec![
                    block("b1", 2, "Registro de aplicación: AGLUTINANTE 127 mg"),
                    block("b2", 4, "Nota: aplicación registrada"),
                ],
            ),
        ];
        let r = run("fev1", "factura.pdf", &items, &docs, &[]);
        assert_eq!(r.items[0].status, "soportado");
        assert_eq!(r.items[0].support[0].pages, vec![2]);
        assert_eq!(r.items[1].status, "sin_soporte");
        assert_eq!(r.summary.supported, 1);
        assert_eq!(r.summary.without_support, 1);
    }

    #[test]
    fn sinonimos_configurables_por_caso() {
        let items = vec![item(1, "1000", "AGLUTINANTE EJEMPLO")];
        let docs = vec![(
            DocMeta {
                doc_id: "doc1".into(),
                file_name: "soporte.pdf".into(),
                pages: 2,
                blocks: 3,
            },
            vec![block("b1", 1, "se aplica COAGULANTE en la zona")],
        )];
        // sin sinónimos: no hay soporte
        let r = run("fev1", "factura.pdf", &items, &docs, &[]);
        assert_eq!(r.items[0].status, "sin_soporte");
        // con sinónimo configurado: sí
        let syn = vec![("aglutinante".to_string(), "coagulante".to_string())];
        let r = run("fev1", "factura.pdf", &items, &docs, &syn);
        assert_eq!(r.items[0].status, "soportado");
    }
}
