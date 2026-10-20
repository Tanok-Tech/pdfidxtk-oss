//! Indexado: extrae el PDF y publica el paquete de artefactos en el store.

use anyhow::{Context, Result};
use regex::Regex;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::Instant;

use crate::extract::{extract_document, LineData, PageData};
use crate::model::*;
use crate::model::RegistryDoc;
use crate::store::{hash_file, now_iso, DocLock, Store, TOOL_VERSION};

pub enum Outcome {
    Indexed(IndexedDoc),
    Skipped(String),
}

struct Heading {
    page: usize,
    text: String,
    level: u8,
}

pub fn index_document(
    store: &Store,
    path: &Path,
    force: bool,
    wait: bool,
    ocr: bool,
    max_pages: usize,
) -> Result<Outcome> {
    let started = Instant::now();
    let (sha256, bytes) = hash_file(path)?;
    let doc_id: String = sha256.chars().take(12).collect();
    let file_name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| doc_id.clone());

    // Idempotencia rápida antes de tomar el lock.
    if !force {
        if let Ok(reg) = store.load_registry() {
            if let Some(d) = reg.docs.get(&doc_id) {
                if d.status == "ready" {
                    return Ok(Outcome::Skipped(file_name));
                }
            }
        }
    }

    let _lock: DocLock = store.lock_doc(&doc_id, wait)?;

    // Re-chequeo dentro del lock (otra sesión pudo terminarlo mientras esperábamos).
    if !force {
        if let Ok(reg) = store.load_registry() {
            if let Some(d) = reg.docs.get(&doc_id) {
                if d.status == "ready" {
                    return Ok(Outcome::Skipped(file_name));
                }
            }
        }
    }

    let mut extracted = extract_document(path)?;

    let tmp = store.tmp_doc_dir(&doc_id);
    if tmp.exists() {
        fs::remove_dir_all(&tmp).ok();
    }
    fs::create_dir_all(tmp.join("chunks"))?;

    // ── OCR de páginas escaneadas (tesseract, local) ──────────────────────
    let mut ocr_engine_label: Option<String> = None;
    let mut ocr_missing: usize = 0;
    if ocr {
        let candidates: Vec<usize> = extracted
            .page_data
            .iter()
            .filter(|p| p.needs_ocr())
            .map(|p| p.page)
            .collect();
        if !candidates.is_empty() {
            match crate::ocr::detect() {
                Some(engine) => {
                    let doc = pdf_oxide::document::PdfDocument::open(path)
                        .with_context(|| "no se pudo reabrir el PDF para OCR")?;
                    for page_no in candidates {
                        let height = doc
                            .get_page_media_box(page_no - 1)
                            .map(|(_, lly, _, ury)| ury - lly)
                            .unwrap_or(792.0);
                        let png = crate::render::render_page(
                            path,
                            page_no,
                            300,
                            &tmp.join("renders"),
                        )?;
                        let lines = crate::ocr::ocr_png(&engine, &png, height, 300)?;
                        crate::extract::merge_ocr_lines(&mut extracted, page_no, lines);
                    }
                    ocr_engine_label = Some(engine.label());
                }
                None => {
                    ocr_missing = extracted
                        .page_data
                        .iter()
                        .filter(|p| p.needs_ocr())
                        .count();
                }
            }
        }
    }

    let mut built = build_artifacts(
        &doc_id,
        &file_name,
        path,
        &sha256,
        bytes,
        &extracted,
        max_pages,
    )?;
    built.meta.ocr_engine = ocr_engine_label;
    if ocr_missing > 0 {
        built.meta.warnings.push(format!(
            "{ocr_missing} página(s) necesitan OCR pero tesseract no está disponible en el PATH"
        ));
    }

    fs::write(tmp.join("meta.json"), serde_json::to_string_pretty(&built.meta)?)?;
    fs::write(tmp.join("tree.json"), serde_json::to_string_pretty(&built.tree)?)?;
    fs::write(
        tmp.join("tree.raw.json"),
        serde_json::to_string_pretty(&built.tree_raw)?,
    )?;
    write_blocks(&tmp.join("blocks.jsonl"), &built.blocks)?;
    write_tables(&tmp.join("tables.jsonl"), &extracted.page_data)?;
    fs::write(tmp.join("full.md"), &built.full_md)?;
    for (name, content) in &built.chunks {
        fs::write(tmp.join("chunks").join(name), content)?;
    }

    store.publish_doc_dir(&doc_id, &tmp)?;

    store.set_registry_doc(
        &doc_id,
        RegistryDoc {
            file_name: file_name.clone(),
            path: path
                .canonicalize()
                .unwrap_or_else(|_| path.to_path_buf())
                .display()
                .to_string(),
            sha256: sha256.clone(),
            bytes,
            pages: extracted.pages,
            status: "ready".to_string(),
            indexed_at: now_iso(),
        },
    )?;

    let mut artifacts = vec![
        "meta.json".to_string(),
        "tree.json".to_string(),
        "tree.raw.json".to_string(),
        "blocks.jsonl".to_string(),
        "dates.idx".to_string(),
        "tables.jsonl".to_string(),
        "full.md".to_string(),
        "chunks/".to_string(),
    ];
    if store.doc_dir(&doc_id).join("renders").exists() {
        artifacts.push("renders/".to_string());
    }
    artifacts.sort();

    Ok(Outcome::Indexed(IndexedDoc {
        doc_id,
        file_name,
        pages: extracted.pages,
        status: "ready".to_string(),
        native_pages: built.meta.extraction.native_pages,
        ocr_required_pages: built.meta.extraction.ocr_required_pages.clone(),
        chars: built.meta.extraction.total_chars,
        elapsed_ms: started.elapsed().as_millis(),
        artifacts,
    }))
}

struct Built {
    meta: MetaFile,
    tree: TreeFile,
    tree_raw: TreeFile,
    blocks: Vec<Block>,
    full_md: String,
    chunks: Vec<(String, String)>,
}

fn build_artifacts(
    doc_id: &str,
    file_name: &str,
    path: &Path,
    sha256: &str,
    bytes: u64,
    extracted: &crate::extract::ExtractedDoc,
    max_pages: usize,
) -> Result<Built> {
    let pages = &extracted.page_data;

    // ── bloques (una línea = un bloque) ─────────────────────────────────────
    let mut blocks: Vec<Block> = Vec::new();
    let mut seq = 0usize;
    for page in pages {
        for line in &page.lines {
            if line.text.trim().is_empty() {
                continue;
            }
            seq += 1;
            blocks.push(Block {
                id: format!("b{seq:04}"),
                node: String::new(), // se asigna en el árbol
                page: page.page,
                kind: "line".to_string(),
                source: line.source.clone(),
                bbox: Some(line.bbox),
                font_size: Some(line.font_size),
                text: line.text.clone(),
            });
        }
    }

    // ── líneas repetidas (encabezados/pies de página) ─────────────────────────
    let boiler = boilerplate_keys(pages);

    // ── detección de encabezados ────────────────────────────────────────────
    let heading_re = Regex::new(r"^(?P<num>\d{1,2}(?:\.\d{1,2}){0,3})[\.\)\s\-]+(?P<rest>\S.{0,98})$")?;
    let median = median_font_size(pages);
    let mut headings: Vec<Heading> = Vec::new();
    for page in pages {
        for line in &page.lines {
            if let Some(h) = heading_candidate(line, median, &heading_re) {
                headings.push(Heading {
                    page: page.page,
                    text: h.0,
                    level: h.1,
                });
            }
        }
    }

    // ── árbol ──────────────────────────────────────────────────────────────
    let title = file_name
        .trim_end_matches(".pdf")
        .trim_end_matches(".PDF")
        .to_string();

    let mut tree = TreeFile {
        schema: SCHEMA_TREE.to_string(),
        doc_id: doc_id.to_string(),
        title: title.clone(),
        pages: extracted.pages,
        root: "n0".to_string(),
        nodes: Default::default(),
    };
    tree.nodes.insert(
        "n0".to_string(),
        TreeNode {
            id: "n0".to_string(),
            title,
            level: 0,
            page: [1, extracted.pages.max(1)],
            // la raíz cubre todo el documento
            kind: "section".to_string(),
            children: Vec::new(),
            blocks: Vec::new(),
            summary: None,
        },
    );

    // Si la "densidad" de encabezados es absurda (formularios con muchas líneas
    // en mayúsculas), el árbol pierde valor: caemos a un nodo por página.
    let too_dense = headings.len() > extracted.pages.max(1) * 3;

    let block_node: Vec<String>; // paralelo a `blocks`
    if headings.is_empty() || too_dense {
        // Sin encabezados fiables: un nodo por página (mapa navegable mínimo).
        for page in pages {
            let id = format!("n{}", tree.nodes.len());
            tree.nodes.insert(
                id.clone(),
                TreeNode {
                    id: id.clone(),
                    title: page_label(page, &boiler),
                    level: 1,
                    page: [page.page, page.page],
                    kind: "content".to_string(),
                    children: Vec::new(),
                    blocks: Vec::new(),
                    summary: None,
                },
            );
            tree.nodes.get_mut("n0").unwrap().children.push(id.clone());
        }
        block_node = blocks
            .iter()
            .map(|b| format!("n{}", b.page)) // los nodos de página son n1..nN (page 1 → n1)
            .collect();
    } else {
        // Jerarquía por niveles con pila de ancestros.
        let mut stack: Vec<(u8, String)> = Vec::new(); // (level, node_id)
        for h in &headings {
            let id = format!("n{}", tree.nodes.len());
            while let Some((lvl, _)) = stack.last() {
                if *lvl < h.level {
                    break;
                }
                stack.pop();
            }
            let parent = stack
                .last()
                .map(|(_, pid)| pid.clone())
                .unwrap_or_else(|| "n0".to_string());
            tree.nodes.insert(
                id.clone(),
                TreeNode {
                    id: id.clone(),
                    title: h.text.clone(),
                    level: h.level,
                    page: [h.page, h.page],
                    kind: "section".to_string(),
                    children: Vec::new(),
                    blocks: Vec::new(),
                    summary: None,
                },
            );
            tree.nodes.get_mut(&parent).unwrap().children.push(id.clone());
            stack.push((h.level, id));
        }
        // Asignar cada bloque (en orden de lectura) al último encabezado visto.
        let mut current = "n0".to_string();
        let mut heading_cursor = 0usize;
        let mut out: Vec<String> = Vec::with_capacity(blocks.len());
        for page in pages {
            for line in &page.lines {
                if line.text.trim().is_empty() {
                    continue;
                }
                // ¿esta línea ES un encabezado? (comparación por (página, texto))
                if heading_cursor < headings.len()
                    && headings[heading_cursor].page == page.page
                    && headings[heading_cursor].text.trim() == line.text.trim()
                {
                    current = format!("n{}", heading_cursor + 1); // n1..nN
                    heading_cursor += 1;
                }
                out.push(current.clone());
            }
        }
        block_node = out;
    }

    // Asignar bloques a nodos y construir listas.
    for (block, node_id) in blocks.iter_mut().zip(block_node.iter()) {
        block.node = node_id.clone();
        if let Some(node) = tree.nodes.get_mut(node_id) {
            node.blocks.push(block.id.clone());
        }
    }

    // Rango de páginas por nodo (a partir de sus bloques).
    let page_of_block: std::collections::HashMap<&str, usize> =
        blocks.iter().map(|b| (b.id.as_str(), b.page)).collect();
    let mut node_ranges: Vec<(String, [usize; 2])> = Vec::new();
    for (id, node) in tree.nodes.iter() {
        let mut min = usize::MAX;
        let mut max = 0usize;
        for bid in &node.blocks {
            if let Some(&p) = page_of_block.get(bid.as_str()) {
                min = min.min(p);
                max = max.max(p);
            }
        }
        if min != usize::MAX {
            node_ranges.push((id.clone(), [min, max]));
        }
    }
    for (id, range) in node_ranges {
        if id == "n0" {
            continue;
        }
        if let Some(n) = tree.nodes.get_mut(&id) {
            n.page = range;
        }
    }
    if let Some(root) = tree.nodes.get_mut("n0") {
        root.page = [1, extracted.pages.max(1)];
    }

    // ── optimización del árbol (estilo PageIndex) ─────────────────────────
    let tree_raw = tree.clone();
    let (tree, opt_stats) = crate::tree::optimize(&tree_raw, max_pages);

    // ── meta con controles de integridad ───────────────────────────────────
    let mut page_stats = Vec::new();
    let mut native_pages = 0usize;
    let mut empty_pages = Vec::new();
    let mut ocr_required = Vec::new();
    let mut garbled_pages = Vec::new();
    let mut total_chars = 0usize;
    let mut total_spans = 0usize;
    let mut total_tables = 0usize;
    let mut total_images = 0usize;

    let mut ocr_done: Vec<usize> = Vec::new();
    let failure_by_page: std::collections::HashMap<usize, String> = extracted
        .failed_pages
        .iter()
        .map(|f| (f.page, f.error.clone()))
        .collect();
    for page in pages {
        let chars: usize = page.lines.iter().map(|l| l.text.chars().count()).sum();
        let mode = if chars > 0 && page.ocr {
            ocr_done.push(page.page);
            "ocr"
        } else if chars == 0 {
            empty_pages.push(page.page);
            "empty"
        } else if chars < 20 {
            "sparse"
        } else {
            native_pages += 1;
            "native"
        };
        if chars < 20 && page.images > 0 {
            ocr_required.push(page.page);
        }
        if page.garbled_ratio > 0.10 && chars > 0 {
            garbled_pages.push(page.page);
        }
        total_chars += chars;
        total_spans += page.lines.len();
        total_tables += page.tables.len();
        total_images += page.images;
        page_stats.push(PageStat {
            page: page.page,
            mode: mode.to_string(),
            chars,
            spans: page.lines.len(),
            tables: page.tables.len(),
            images: page.images,
            illegible: chars == 0 || page.garbled_ratio > 0.5,
            error: failure_by_page.get(&page.page).cloned(),
        });
    }

    let mut warnings = Vec::new();
    if !empty_pages.is_empty() && !extracted.encrypted {
        // Las páginas NO legibles tienen su propio aviso más abajo: pedirles
        // OCR sería un diagnóstico falso (el OCR no puede abrir lo que no se
        // pudo leer, igual que no puede abrir un PDF cifrado).
        let no_legibles: Vec<usize> = extracted.failed_pages.iter().map(|f| f.page).collect();
        let sin_diagnóstico: Vec<&usize> = empty_pages
            .iter()
            .filter(|p| !no_legibles.contains(p))
            .collect();
        if !sin_diagnóstico.is_empty() {
            warnings.push(format!(
                "{} página(s) sin texto extraíble: {:?}. Requieren OCR o inspección visual.",
                sin_diagnóstico.len(),
                sin_diagnóstico
            ));
        }
    }
    if !ocr_required.is_empty() {
        warnings.push(format!(
            "{} página(s) con texto escaso e imágenes (candidatas a OCR): {:?}",
            ocr_required.len(),
            ocr_required
        ));
    }
    if extracted.encoding_repairs > 0 {
        warnings.push(format!(
            "{} línea(s) con anomalía de encoding (mojibake reparado o parcial).              Revisar términos acentuados antes de citar.",
            extracted.encoding_repairs
        ));
    }
    if !garbled_pages.is_empty() {
        warnings.push(format!(
            "{} página(s) con posible texto ilegible (garbled ratio > 10%): {:?}",
            garbled_pages.len(),
            garbled_pages
        ));
    }
    if !extracted.failed_pages.is_empty() {
        let pages: Vec<usize> = extracted.failed_pages.iter().map(|f| f.page).collect();
        warnings.push(format!(
            "{} página(s) NO se pudieron leer (quedan declaradas, no se pierden en silencio): {:?}. \
             Verificar con render o inspección visual del PDF original.",
            pages.len(),
            pages
        ));
        for f in &extracted.failed_pages {
            warnings.push(format!("página {}: {}", f.page, f.error));
        }
    }
    if extracted.encrypted {
        warnings.push(format!(
            "El PDF está CIFRADO con contraseña: no se pudo leer el contenido de ninguna de sus \
             {} página(s). No es un documento escaneado y el OCR no puede ayudar: \
             hace falta el PDF desprotegido o su contraseña.",
            extracted.pages
        ));
    }

    let meta = MetaFile {
        schema: SCHEMA_META.to_string(),
        doc_id: doc_id.to_string(),
        status: "ready".to_string(),
        tool_version: TOOL_VERSION.to_string(),
        created_at: now_iso(),
        ocr_engine: None,
        tree_opt: Some(opt_stats),
        source: SourceInfo {
            path: path
                .canonicalize()
                .unwrap_or_else(|_| path.to_path_buf())
                .display()
                .to_string(),
            file_name: file_name.to_string(),
            sha256: sha256.to_string(),
            bytes,
        },
        pages: extracted.pages,
        page_stats,
        extraction: ExtractionSummary {
            native_pages,
            empty_pages,
            ocr_required_pages: ocr_required,
            ocr_pages: ocr_done.clone(),
            total_chars,
            total_spans,
            total_tables,
            total_images,
            garbled_pages,
            failed_pages: extracted
                .failed_pages
                .iter()
                .map(|f| f.page)
                .collect(),
            encrypted: extracted.encrypted,
        },
        warnings,
    };

    // ── full.md + chunks (formato compatible con el paquete de auditoría) ──
    let full_md = render_marks(pages);
    let mut chunks = Vec::new();
    const CHUNK: usize = 10;
    let mut start = 1usize;
    while start <= extracted.pages.max(1) {
        let end = (start + CHUNK - 1).min(extracted.pages.max(1));
        let slice: Vec<PageData> = pages
            .iter()
            .filter(|p| p.page >= start && p.page <= end)
            .cloned()
            .collect();
        let content = render_marks(&slice);
        chunks.push((format!("p{start:04}-p{end:04}.md"), content));
        start = end + 1;
    }

    Ok(Built {
        meta,
        tree,
        tree_raw,
        blocks,
        full_md,
        chunks,
    })
}

fn render_marks(pages: &[PageData]) -> String {
    let mut out = String::new();
    for page in pages {
        out.push_str(&format!("<!-- PAGINA {} -->\n\n## Pagina {}\n\n", page.page, page.page));
        for line in &page.lines {
            out.push_str(&line.text);
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

fn median_font_size(pages: &[PageData]) -> f32 {
    let mut sizes: Vec<f32> = pages
        .iter()
        .flat_map(|p| p.lines.iter().map(|l| l.font_size))
        .filter(|s| *s > 0.0)
        .collect();
    if sizes.is_empty() {
        return 10.0;
    }
    sizes.sort_by(|a, b| a.partial_cmp(b).unwrap());
    sizes[sizes.len() / 2]
}

/// Devuelve (texto, nivel) si la línea parece encabezado.
fn normalize_key(text: &str) -> String {
    // Colapsa espacios y neutraliza números de página: "Página 9 de 1378" y
    // "Página 17 de 1378" son la misma línea de pie → boilerplate.
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = String::with_capacity(collapsed.len());
    let mut prev_digit = false;
    for ch in collapsed.to_lowercase().chars() {
        if ch.is_ascii_digit() {
            if !prev_digit {
                out.push('#');
            }
            prev_digit = true;
        } else {
            prev_digit = false;
            out.push(ch);
        }
    }
    out.chars().take(80).collect()
}

/// Líneas que se repiten en muchas páginas (encabezados/pies institucionales):
/// no sirven como título de una página. Umbral: aparecen en ≥10% de las páginas
/// (mínimo 3) — un documento de 3 páginas nunca marca nada.
fn boilerplate_keys(pages: &[crate::extract::PageData]) -> std::collections::HashSet<String> {
    use std::collections::{HashMap, HashSet};
    let mut counts: HashMap<String, usize> = HashMap::new();
    let n = pages.len().max(1);
    for page in pages {
        let mut seen: HashSet<String> = HashSet::new();
        for line in &page.lines {
            let t = line.text.trim();
            if t.chars().count() < 8 {
                continue;
            }
            let k = normalize_key(t);
            if seen.insert(k.clone()) {
                *counts.entry(k).or_default() += 1;
            }
        }
    }
    let threshold = (n / 10).max(3);
    counts
        .into_iter()
        .filter(|(_, c)| *c >= threshold)
        .map(|(k, _)| k)
        .collect()
}

/// Título legible para un nodo de página cuando no hay encabezados fiables:
/// la primera línea con texto real (evita numeración suelta y líneas de dígitos).
fn page_label(page: &crate::extract::PageData, boiler: &std::collections::HashSet<String>) -> String {
    for line in &page.lines {
        let t = line.text.trim();
        let chars = t.chars().count();
        if chars < 8 || boiler.contains(&normalize_key(t)) {
            continue;
        }
        let letters = t.chars().filter(|c| c.is_alphabetic()).count();
        if letters < 3 || (letters as f32 / chars as f32) < 0.45 {
            continue;
        }
        let mut out: String = t.chars().take(70).collect();
        if chars > 70 {
            out.push('…');
        }
        return out;
    }
    format!("Pagina {}", page.page)
}

fn heading_candidate(line: &LineData, median: f32, re: &Regex) -> Option<(String, u8)> {
    let text = line.text.trim();
    let chars = text.chars().count();
    if chars < 2 || chars > 110 {
        return None;
    }

    let letters = text.chars().filter(|c| c.is_alphabetic()).count();
    let upper = text.chars().filter(|c| c.is_uppercase()).count();
    let upper_ratio = if letters > 0 { upper as f32 / letters as f32 } else { 0.0 };
    let digit_tokens = text
        .split_whitespace()
        .filter(|t| t.chars().any(|c| c.is_ascii_digit()))
        .count();

    // ── numeración tipo "3.1.2 Título" → nivel por profundidad ─────────────
    if let Some(caps) = re.captures(text) {
        let num = &caps["num"];
        let rest = &caps["rest"];
        // El resto debe empezar con letra (mata filas tipo "1 992511 ...", "2 $63.000 ...")
        if !rest.chars().next().map(|c| c.is_alphabetic()).unwrap_or(false) {
            return None;
        }
        // Y no puede traer 2+ códigos con dígitos ("7DS005 7DS005 ...")
        let rest_codes = rest
            .split_whitespace()
            .filter(|t| t.chars().any(|c| c.is_ascii_digit()))
            .count();
        if rest_codes >= 2 || rest.split_whitespace().count() > 12 {
            return None;
        }
        let level = (num.matches('.').count() as u8).saturating_add(1).min(6);
        return Some((text.to_string(), level));
    }

    // ── MAYÚSCULAS de sección: corto, sin dígitos, ratio alto ──────────────
    let words = text.split_whitespace().count();
    if upper_ratio > 0.90
        && chars <= 45
        && digit_tokens == 0
        && (words >= 2 || (words == 1 && chars <= 20 && line.font_size >= median * 1.15))
    {
        return Some((text.to_string(), 1));
    }

    // ── negrita grande en texto mixto, sin "Etiqueta: valor" ───────────────
    if line.bold
        && line.font_size >= median * 1.30
        && chars <= 70
        && upper_ratio <= 0.65
        && digit_tokens == 0
        && !text.contains(": ")
    {
        return Some((text.to_string(), 1));
    }

    None
}

fn write_blocks(path: &Path, blocks: &[Block]) -> Result<()> {
    let mut f = fs::File::create(path)?;
    for b in blocks {
        let line = serde_json::to_string(b)?;
        f.write_all(line.as_bytes())?;
        f.write_all(b"\n")?;
    }
    Ok(())
}

fn write_tables(path: &Path, pages: &[PageData]) -> Result<()> {
    #[derive(serde::Serialize)]
    struct TableOut<'a> {
        page: usize,
        table_index: usize,
        column_count: usize,
        row_count: usize,
        rows: &'a Vec<Vec<String>>,
    }
    let mut f = fs::File::create(path)?;
    for page in pages {
        for (i, t) in page.tables.iter().enumerate() {
            let cols = t.rows.iter().map(|r| r.len()).max().unwrap_or(0);
            let out = TableOut {
                page: page.page,
                table_index: i + 1,
                column_count: cols,
                row_count: t.rows.len(),
                rows: &t.rows,
            };
            f.write_all(serde_json::to_string(&out)?.as_bytes())?;
            f.write_all(b"\n")?;
        }
    }
    Ok(())
}
