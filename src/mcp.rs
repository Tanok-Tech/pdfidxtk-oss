//! `pitk serve --mcp` — servidor MCP mínimo sobre stdio (JSON-RPC 2.0).
//!
//! Sin estado entre llamadas: cada tool reutiliza el store en disco. Las rutas
//! solicitadas se resuelven y DEBEN quedar dentro del workspace; cualquier
//! intento de salir se rechaza (aislamiento entre workspaces también por aquí).

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use crate::cross;
use crate::fev;
use crate::index;
use crate::model;
use crate::query;
use crate::render;
use crate::store::{list_pdfs, Store, TOOL_VERSION};

pub fn serve(workspace: PathBuf) -> Result<()> {
    let store = Store::open(&workspace)?;
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                writeln!(
                    stdout,
                    "{}",
                    json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":format!("parse error: {e}")}})
                )?;
                stdout.flush()?;
                continue;
            }
        };

        let id = req.get("id").cloned().unwrap_or(Value::Null);
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = req.get("params").cloned().unwrap_or(Value::Null);

        let resp = match method {
            "initialize" => Some(ok(
                id,
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "pdfidxtk", "version": TOOL_VERSION}
                }),
            )),
            "notifications/initialized" | "notifications/cancelled" => None,
            "ping" => Some(ok(id, json!({}))),
            "tools/list" => Some(ok(id, json!({"tools": tools()}))),
            "tools/call" => Some(call_tool(&store, id, &params)),
            _ => Some(err(id, -32601, format!("method not found: {method}"))),
        };

        if let Some(resp) = resp {
            writeln!(stdout, "{}", serde_json::to_string(&resp)?)?;
            stdout.flush()?;
        }
    }
    Ok(())
}

fn ok(id: Value, result: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":result})
}

fn err(id: Value, code: i64, message: String) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}

fn tools() -> Vec<Value> {
    vec![
        json!({
            "name": "pitk_info",
            "description": "Estado del workspace: documentos indexados, páginas, caracteres y tablas. Úsalo primero para saber si hace falta indexar.",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false}
        }),
        json!({
            "name": "pitk_index",
            "description": "Descompone todos los PDFs del workspace en el store (.docidx): árbol, bloques por página, tablas, fechas y meta con controles de integridad. Idempotente.",
            "inputSchema": {"type": "object", "properties": {"force": {"type": "boolean"}}, "additionalProperties": false}
        }),
        json!({
            "name": "pitk_fev",
            "description": "Extrae y reconcilia los ítems de un documento tabular de ítems/precios (FEV: factura, cuenta de cobro, estado de cuenta): cantidad x unitario = total por ítem, suma contra total declarado, faltantes/duplicados por grupo.",
            "inputSchema": {
                "type": "object",
                "properties": {"pdf": {"type": "string", "description": "ruta relativa al workspace"}, "force": {"type": "boolean"}},
                "required": ["pdf"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "pitk_dates",
            "description": "Páginas que mencionan una fecha o rango, por búsqueda binaria sobre el índice cronológico. Formatos: 2026-09-02, 02/09/2026, 2026-09, 2026-01-01..2026-12-31.",
            "inputSchema": {
                "type": "object",
                "properties": {"pdf": {"type": "string"}, "query": {"type": "string"}},
                "required": ["pdf", "query"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "pitk_search",
            "description": "Busca términos (sin acentos) en los bloques del documento. Con receipt=true genera un recibo reproducible con hash del corpus para pruebas de ausencia.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pdf": {"type": "string"},
                    "terms": {"type": "array", "items": {"type": "string"}},
                    "receipt": {"type": "boolean"}
                },
                "required": ["pdf", "terms"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "pitk_cross",
            "description": "Cruza los ítems de un documento tabular (FEV: factura, cuenta de cobro, estado de cuenta) contra los documentos de soporte del caso (anexos, actas, contratos...): por ítem, en qué documento y páginas aparece soporte, y qué ítems quedan sin soporte.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "fev": {"type": "string"},
                    "docs": {"type": "array", "items": {"type": "string"}}
                },
                "required": ["fev", "docs"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "pitk_optimize",
            "description": "Reoptimiza el árbol del documento: fusiona nodos hermanos para que ningún nodo cubra más de max_pages páginas (menos nodos que leer para el agente). 0 restaura el árbol crudo.",
            "inputSchema": {
                "type": "object",
                "properties": {"pdf": {"type": "string"}, "max_pages": {"type": "integer", "minimum": 0}},
                "required": ["pdf"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "pitk_render",
            "description": "Renderiza una página a PNG (300 dpi por defecto) para verificar manuscritos o contenido visual que el texto no captura. Devuelve la ruta del PNG.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pdf": {"type": "string"},
                    "page": {"type": "integer", "minimum": 1},
                    "dpi": {"type": "integer", "minimum": 50, "maximum": 600}
                },
                "required": ["pdf", "page"],
                "additionalProperties": false
            }
        }),
    ]
}

fn call_tool(store: &Store, id: Value, params: &Value) -> Value {
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));

    match dispatch(store, name, &args) {
        Ok(v) => ok(
            id,
            json!({
                "content": [{"type": "text", "text": serde_json::to_string(&v).unwrap_or_default()}],
                "isError": false
            }),
        ),
        Err(e) => ok(
            id,
            json!({
                "content": [{"type": "text", "text": format!("error: {e:#}")}],
                "isError": true
            }),
        ),
    }
}

fn resolve_ws(store: &Store, p: &str) -> Result<PathBuf> {
    let raw = PathBuf::from(p);
    // Rechazo temprano y explícito de cualquier intento de salir del workspace.
    if raw
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        bail!("ruta fuera del workspace (rechazada): {p}");
    }
    let path = if raw.is_absolute() {
        raw
    } else {
        store.workspace.join(raw)
    };
    let canon = path
        .canonicalize()
        .with_context(|| format!("no existe: {}", path.display()))?;
    if !canon.starts_with(&store.workspace) {
        bail!("ruta fuera del workspace (rechazada): {}", p);
    }
    Ok(canon)
}

fn ensure_indexed(store: &Store, pdf: &Path) -> Result<String> {
    let (sha, _) = crate::store::hash_file(pdf)?;
    let doc_id: String = sha.chars().take(12).collect();
    if !store.doc_dir(&doc_id).join("blocks.jsonl").exists() {
        index::index_document(store, pdf, false, false, true, crate::tree::DEFAULT_MAX_PAGES)?;
    }
    Ok(doc_id)
}

fn load_blocks(path: &Path) -> Result<Vec<model::Block>> {
    let raw = std::fs::read_to_string(path)?;
    let mut out = Vec::new();
    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }
        out.push(serde_json::from_str::<model::Block>(line)?);
    }
    Ok(out)
}

fn dispatch(store: &Store, name: &str, args: &Value) -> Result<Value> {
    match name {
        "pitk_info" => {
            let reg = store.load_registry()?;
            let mut docs = Vec::new();
            for (doc_id, d) in &reg.docs {
                let meta: Option<model::MetaFile> =
                    std::fs::read_to_string(store.doc_dir(doc_id).join("meta.json"))
                        .ok()
                        .and_then(|raw| serde_json::from_str(&raw).ok());
                docs.push(json!({
                    "doc_id": doc_id,
                    "file_name": d.file_name,
                    "status": d.status,
                    "pages": d.pages,
                    "native_pages": meta.as_ref().map(|m| m.extraction.native_pages),
                    "total_chars": meta.as_ref().map(|m| m.extraction.total_chars),
                    "total_tables": meta.as_ref().map(|m| m.extraction.total_tables),
                    "ocr_required_pages": meta.as_ref().map(|m| m.extraction.ocr_required_pages.clone()),
                    "warnings": meta.as_ref().map(|m| m.warnings.clone()),
                }));
            }
            Ok(json!({
                "workspace": store.workspace.display().to_string(),
                "account": reg.account,
                "docs": docs
            }))
        }
        "pitk_index" => {
            let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
            let ocr_on = args.get("ocr").and_then(|v| v.as_bool()).unwrap_or(true);
            let max_pages = args
                .get("max_pages")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize)
                .unwrap_or(crate::tree::DEFAULT_MAX_PAGES);
            let pdfs = list_pdfs(&store.workspace)?;
            let mut indexed = Vec::new();
            let mut skipped = Vec::new();
            for p in &pdfs {
                match index::index_document(store, p, force, false, ocr_on, max_pages)? {
                    index::Outcome::Indexed(d) => indexed.push(json!({
                        "doc_id": d.doc_id, "file_name": d.file_name, "pages": d.pages,
                        "native_pages": d.native_pages, "chars": d.chars,
                        "ocr_required_pages": d.ocr_required_pages
                    })),
                    index::Outcome::Skipped(n) => skipped.push(n),
                }
            }
            Ok(json!({"indexed": indexed, "skipped": skipped, "total_pdfs": pdfs.len()}))
        }
        "pitk_fev" => {
            let pdf = resolve_ws(store, &str_arg(args, "pdf")?)?;
            let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
            let doc_id = ensure_indexed(store, &pdf)?;
            let fev_path = store.doc_dir(&doc_id).join("fev.json");
            let cache_ok = fev_path
                .exists()
                && std::fs::read_to_string(&fev_path)
                    .ok()
                    .and_then(|raw| serde_json::from_str::<fev::FevReport>(&raw).ok())
                    .map(|r| r.tool_version == crate::store::TOOL_VERSION)
                    .unwrap_or(false);
            let report: fev::FevReport = if cache_ok && !force {
                serde_json::from_str(&std::fs::read_to_string(&fev_path)?)?
            } else {
                let blocks = load_blocks(&store.doc_dir(&doc_id).join("blocks.jsonl"))?;
                let file_name = pdf
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let r = fev::parse_blocks(&doc_id, &file_name, &blocks);
                std::fs::write(&fev_path, serde_json::to_string_pretty(&r)?)?;
                r
            };
            Ok(serde_json::to_value(report)?)
        }
        "pitk_dates" => {
            let pdf = resolve_ws(store, &str_arg(args, "pdf")?)?;
            let q = str_arg(args, "query")?;
            let doc_id = ensure_indexed(store, &pdf)?;
            let doc_dir = store.doc_dir(&doc_id);
            let idx: query::DatesIndex =
                serde_json::from_str(&std::fs::read_to_string(doc_dir.join("dates.idx"))?)?;
            let (start, end) = query::parse_date_query(&q)?;
            let days: Vec<&query::DateEntry> = query::query_dates(&idx, &start, &end);
            let mut pages: Vec<usize> = days.iter().flat_map(|d| d.pages.clone()).collect();
            pages.sort_unstable();
            pages.dedup();
            let total_hits: usize = days.iter().map(|d| d.hits).sum();
            Ok(json!({
                "doc_id": doc_id,
                "query": {"start": start, "end": end},
                "days": days,
                "pages": pages,
                "total_hits": total_hits
            }))
        }
        "pitk_search" => {
            let pdf = resolve_ws(store, &str_arg(args, "pdf")?)?;
            let terms: Vec<String> = args
                .get("terms")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            if terms.is_empty() {
                bail!("terms no puede estar vacío");
            }
            let receipt = args
                .get("receipt")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let doc_id = ensure_indexed(store, &pdf)?;
            let doc_dir = store.doc_dir(&doc_id);
            let blocks_path = doc_dir.join("blocks.jsonl");
            let blocks = load_blocks(&blocks_path)?;
            let results = query::search_blocks(&blocks, &terms);
            let mut receipt_path = None;
            if receipt {
                let meta: Option<model::MetaFile> =
                    std::fs::read_to_string(doc_dir.join("meta.json"))
                        .ok()
                        .and_then(|raw| serde_json::from_str(&raw).ok());
                let corpus = std::fs::read_to_string(&blocks_path)?;
                let qhash: String = query::hash_text(&terms.join("|")).chars().take(12).collect();
                let r = query::SearchReceipt {
                    schema: "pitk/search-receipt/1".to_string(),
                    created_at: crate::store::now_iso(),
                    doc_id: doc_id.clone(),
                    file_name: pdf
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string(),
                    source_sha256: meta.as_ref().map(|m| m.source.sha256.clone()).unwrap_or_default(),
                    pages: meta.as_ref().map(|m| m.pages).unwrap_or(0),
                    blocks: blocks.len(),
                    corpus_sha256: query::hash_text(&corpus),
                    query: terms.clone(),
                    results: results.clone(),
                };
                let dir = doc_dir.join("receipts");
                std::fs::create_dir_all(&dir)?;
                let path = query::unique_receipt_path(&dir, &qhash);
                std::fs::write(&path, serde_json::to_string_pretty(&r)?)?;
                receipt_path = Some(path.display().to_string());
            }
            Ok(json!({"doc_id": doc_id, "terms": results, "receipt": receipt_path}))
        }
        "pitk_cross" => {
            let fev_pdf = resolve_ws(store, &str_arg(args, "fev")?)?;
            let docs: Vec<PathBuf> = args
                .get("docs")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<&str>>())
                .unwrap_or_default()
                .into_iter()
                .map(|s| resolve_ws(store, s))
                .collect::<Result<Vec<_>>>()?;
            if docs.is_empty() {
                bail!("docs no puede estar vacío");
            }
            let fev_doc_id = ensure_indexed(store, &fev_pdf)?;
            let fev_blocks = load_blocks(&store.doc_dir(&fev_doc_id).join("blocks.jsonl"))?;
            let fev_name = fev_pdf
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let fev_report = fev::parse_blocks(&fev_doc_id, &fev_name, &fev_blocks);

            let mut loaded = Vec::new();
            for d in &docs {
                let doc_id = ensure_indexed(store, d)?;
                let doc_dir = store.doc_dir(&doc_id);
                let blocks = load_blocks(&doc_dir.join("blocks.jsonl"))?;
                let meta: Option<model::MetaFile> = std::fs::read_to_string(doc_dir.join("meta.json"))
                    .ok()
                    .and_then(|raw| serde_json::from_str(&raw).ok());
                loaded.push((
                    cross::DocMeta {
                        doc_id,
                        file_name: d
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string(),
                        pages: meta.as_ref().map(|m| m.pages).unwrap_or(0),
                        blocks: blocks.len(),
                    },
                    blocks,
                ));
            }
            let synonyms = cross::load_synonyms(&store.store_dir());
            let report =
                cross::run(&fev_doc_id, &fev_name, &fev_report.items, &loaded, &synonyms);
            let out_path = store.doc_dir(&fev_doc_id).join("cross.json");
            std::fs::write(&out_path, serde_json::to_string_pretty(&report)?)?;
            Ok(serde_json::to_value(report)?)
        }
        "pitk_optimize" => {
            let pdf = resolve_ws(store, &str_arg(args, "pdf")?)?;
            let max_pages = args
                .get("max_pages")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize)
                .unwrap_or(crate::tree::DEFAULT_MAX_PAGES);
            let doc_id = ensure_indexed(store, &pdf)?;
            let doc_dir = store.doc_dir(&doc_id);
            let raw_path = doc_dir.join("tree.raw.json");
            let tree_path = doc_dir.join("tree.json");
            let base_path = if raw_path.exists() { raw_path } else { tree_path.clone() };
            let base: model::TreeFile =
                serde_json::from_str(&std::fs::read_to_string(&base_path)?)?;
            let (opt, st) = crate::tree::optimize(&base, max_pages);
            std::fs::write(&tree_path, serde_json::to_string_pretty(&opt)?)?;
            let meta_path = doc_dir.join("meta.json");
            let mut meta: model::MetaFile =
                serde_json::from_str(&std::fs::read_to_string(&meta_path)?)?;
            meta.tree_opt = Some(st.clone());
            std::fs::write(&meta_path, serde_json::to_string_pretty(&meta)?)?;
            Ok(json!({"doc_id": doc_id, "stats": st}))
        }
        "pitk_render" => {
            let pdf = resolve_ws(store, &str_arg(args, "pdf")?)?;
            let page = args
                .get("page")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize)
                .unwrap_or(0);
            if page == 0 {
                bail!("page es 1-based");
            }
            let dpi = args
                .get("dpi")
                .and_then(|v| v.as_u64())
                .map(|n| n as u32)
                .unwrap_or(300);
            let doc_id = ensure_indexed(store, &pdf)?;
            let out = render::render_page(
                &pdf,
                page,
                dpi,
                &store.doc_dir(&doc_id).join("renders"),
            )?;
            Ok(json!({"doc_id": doc_id, "page": page, "dpi": dpi, "path": out.display().to_string()}))
        }
        other => bail!("tool desconocida: {other}"),
    }
}

fn str_arg(args: &Value, key: &str) -> Result<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .with_context(|| format!("falta el argumento '{key}'"))
}
