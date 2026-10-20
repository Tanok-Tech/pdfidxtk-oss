//! Modelo de datos del store pdfidxtk.
//!
//! Todos los archivos del store son JSON/JSONL con esquema versionado,
//! legibles por cualquier agente sin necesidad de esta herramienta.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const SCHEMA_REGISTRY: &str = "pitk/registry/1";
pub const SCHEMA_META: &str = "pitk/meta/1";
pub const SCHEMA_TREE: &str = "pitk/tree/1";

// ─────────────────────────────────────────────────────────────────────────────
// registry.json  — lista maestra del workspace (una cuenta = un workspace)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Registry {
    pub schema: String,
    /// Etiqueta de cuenta anclada en `init`. Es la guardia anti-contaminación.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    pub created_at: String,
    pub tool_version: String,
    /// doc_id (hash12) → resumen
    pub docs: BTreeMap<String, RegistryDoc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryDoc {
    pub file_name: String,
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
    pub pages: usize,
    pub status: String, // building | ready | failed
    pub indexed_at: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// docs/<hash12>/meta.json — integridad declarada (controles §3.0 del auditor)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaFile {
    pub schema: String,
    pub doc_id: String,
    pub status: String, // building | ready | failed
    pub tool_version: String,
    pub created_at: String,
    /// motor OCR usado en este indexado, p. ej. "tesseract 5.5.3 (spa+eng)"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ocr_engine: Option<String>,
    /// estadísticas de optimización del árbol (tree.json vs tree.raw.json)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree_opt: Option<crate::tree::OptStats>,
    pub source: SourceInfo,
    pub pages: usize,
    pub page_stats: Vec<PageStat>,
    pub extraction: ExtractionSummary,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceInfo {
    pub path: String,
    pub file_name: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageStat {
    /// 1-based
    pub page: usize,
    /// native | empty | ocr_required
    pub mode: String,
    pub chars: usize,
    pub spans: usize,
    pub tables: usize,
    pub images: usize,
    /// true si la página es sospechosa (0 chars, o glifos ilegibles)
    pub illegible: bool,
    /// Motivo por el que la página no se pudo leer (si aplica).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionSummary {
    pub native_pages: usize,
    pub empty_pages: Vec<usize>,
    pub ocr_required_pages: Vec<usize>,
    /// páginas efectivamente leídas con OCR (source="ocr")
    #[serde(default)]
    pub ocr_pages: Vec<usize>,
    pub total_chars: usize,
    pub total_spans: usize,
    pub total_tables: usize,
    pub total_images: usize,
    /// Páginas con texto extraído pero alta proporción de caracteres de reemplazo
    pub garbled_pages: Vec<usize>,
    /// Páginas que NO se pudieron leer. El documento se indexa igual, pero el
    /// hueco queda declarado: nunca se pierde una página en silencio.
    #[serde(default)]
    pub failed_pages: Vec<usize>,
    /// El PDF está cifrado con contraseña: su contenido no se pudo leer y el
    /// OCR no puede ayudar. Es un caso distinto de "PDF escaneado".
    #[serde(default)]
    pub encrypted: bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// docs/<hash12>/tree.json — árbol n-ario aplanado
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeFile {
    pub schema: String,
    pub doc_id: String,
    pub title: String,
    pub pages: usize,
    pub root: String,
    pub nodes: BTreeMap<String, TreeNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeNode {
    pub id: String,
    pub title: String,
    /// 0 = raíz, 1 = capítulo, 2 = sección...
    pub level: u8,
    /// [página_ini, página_fin] 1-based, ambas inclusivas
    #[serde(rename = "page")]
    pub page: [usize; 2],
    /// section | content | table | image | invoice
    pub kind: String,
    pub children: Vec<String>,
    pub blocks: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// docs/<hash12>/blocks.jsonl — la descomposición cruda (una línea por bloque)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    pub id: String,
    /// nodo del árbol al que pertenece
    pub node: String,
    /// "ocr" cuando el bloque viene de OCR (ausente = texto nativo)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// 1-based
    pub page: usize,
    /// line | table | image
    pub kind: String,
    /// [x0, y0, x1, y1] en puntos PDF
    pub bbox: Option<[f32; 4]>,
    pub font_size: Option<f32>,
    pub text: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Salidas JSON de los comandos
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct InitOutput {
    pub ok: bool,
    pub workspace: String,
    pub store: String,
    pub account: Option<String>,
    pub created: bool,
    pub usage_sheet: String,
    pub pdfs_detected: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct IndexOutput {
    pub ok: bool,
    pub workspace: String,
    pub account: Option<String>,
    pub docs: Vec<IndexedDoc>,
    pub skipped: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct IndexedDoc {
    pub doc_id: String,
    pub file_name: String,
    pub pages: usize,
    pub status: String,
    pub native_pages: usize,
    pub ocr_required_pages: Vec<usize>,
    pub chars: usize,
    pub elapsed_ms: u128,
    pub artifacts: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct InfoOutput {
    pub ok: bool,
    pub workspace: String,
    pub store: String,
    pub account: Option<String>,
    pub docs: Vec<InfoDoc>,
}

#[derive(Debug, Serialize)]
pub struct InfoDoc {
    pub doc_id: String,
    pub file_name: String,
    pub status: String,
    pub pages: usize,
    pub native_pages: usize,
    pub total_chars: usize,
    pub total_tables: usize,
    pub indexed_at: String,
}
