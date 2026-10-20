//! Extracción de PDF con `pdf_oxide` (puro Rust, sin dependencias externas).
//!
//! Salida por página: líneas de texto con bbox/fuente, tablas con celdas,
//! conteo de imágenes y señales de calidad (garbled / texto invisible).

use anyhow::{Context, Result};
use pdf_oxide::document::PdfDocument;
use rayon::prelude::*;
use std::path::Path;

/// Una línea de texto ya agrupada geométricamente.
#[derive(Debug, Clone)]
pub struct LineData {
    pub text: String,
    /// [x0, y0, x1, y1] en coordenadas PDF (origen abajo-izquierda)
    pub bbox: [f32; 4],
    pub font_size: f32,
    pub bold: bool,
    /// true si hubo que reparar mojibake en esta línea
    pub repaired: bool,
    /// Some("ocr") cuando la línea viene de OCR; None = texto nativo
    pub source: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TableData {
    pub rows: Vec<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct PageData {
    /// 1-based
    pub page: usize,
    pub lines: Vec<LineData>,
    pub tables: Vec<TableData>,
    pub images: usize,
    #[allow(dead_code)]
    pub classifier_kind: String,
    pub ocr: bool,
    pub garbled_ratio: f32,
    #[allow(dead_code)]
    pub invisible_text_ratio: f32,
    #[allow(dead_code)]
    pub text_glyph_count: u32,
}

#[derive(Debug, Clone)]
pub struct ExtractedDoc {
    pub pages: usize,
    pub page_data: Vec<PageData>,
    /// Líneas donde hubo que reparar mojibake (pérdida de encoding detectada).
    pub encoding_repairs: usize,
    /// Páginas que no se pudieron leer. Se registran; nunca se pierden en silencio.
    pub failed_pages: Vec<PageFailure>,
    /// El PDF está cifrado y su contenido no se pudo descifrar (contraseña).
    /// En ese caso el texto sale vacío: no es un PDF escaneado, y el OCR no
    /// puede ayudar. El auditor tiene que saber la diferencia.
    pub encrypted: bool,
}

/// Una página que no se pudo leer, con el motivo.
#[derive(Debug, Clone)]
pub struct PageFailure {
    /// 1-based
    pub page: usize,
    pub error: String,
}

/// Extrae el documento completo sin perder páginas.
///
/// `pdf_oxide` 0.3.78 resuelve las páginas sobre un `PdfDocument` compartido
/// con un cache mutable. Con acceso concurrente al mismo handle, la carrera
/// entre el recorrido masivo del árbol (`populate_page_cache`) y las traversals
/// por página produce dos fallos distintos:
///
/// 1. Una página se resuelve a `Null` → el cache queda desalineado y el texto
///    de esa página se pierde **en silencio** (mismo número de páginas, menos
///    caracteres extraídos).
/// 2. La resolución aborta con `Invalid PDF: Page index N not found by scanning`
///    → el documento entero no se indexa.
///
/// Por eso el trabajo se reparte en *fragmentos* y cada fragmento abre **su
/// propio** `PdfDocument`: sin estado mutable compartido, la extracción es
/// determinista y completa. El número de fragmentos sólo afecta al
/// rendimiento, nunca al resultado.
///
/// Encima de eso hay una garantía de completitud: toda página faltante se
/// reintenta en serie con un handle nuevo y, si tampoco se puede leer, queda
/// registrada como error explícito. El documento se indexa igual, pero el
/// auditor ve exactamente qué páginas quedaron fuera — nunca un hueco
/// silencioso.
pub fn extract_document(path: &Path) -> Result<ExtractedDoc> {
    extract_document_with_shards(path, 0)
}

/// Igual que [`extract_document`], pero con el número de fragmentos fijado.
///
/// `shards == 0` lo decide el tamaño del documento. Fijarlo existe para
/// poder demostrar en test que el resultado NO depende del número de
/// fragmentos: es la garantía que hace seguro paralelizar.
pub(crate) fn extract_document_with_shards(path: &Path, shards: usize) -> Result<ExtractedDoc> {
    let (page_count, encrypted) = {
        let doc = PdfDocument::open(path)
            .with_context(|| format!("pdf_oxide no pudo abrir {}", path.display()))?;
        (
            doc.page_count().context("no se pudo leer el número de páginas")?,
            doc.is_encrypted(),
        )
    };

    let ranges = shard_ranges(page_count, shards);
    let shards: Vec<Vec<PageData>> = ranges
        .par_iter()
        .map(|(from, to)| extract_shard(path, *from, *to))
        .collect();

    let mut page_data: Vec<PageData> = Vec::with_capacity(page_count);
    for shard in shards {
        page_data.extend(shard);
    }
    page_data.sort_by_key(|p| p.page);

    // ── Garantía de completitud ────────────────────────────────────────────
    // Toda página 1..=N debe terminar con exactamente un resultado.
    let missing = missing_pages(&page_data, page_count);
    let mut failed_pages: Vec<PageFailure> = Vec::new();
    if !missing.is_empty() {
        let (recovered, still_failing) = retry_pages(path, &missing);
        page_data.extend(recovered);
        // Las páginas irrecuperables conservan su lugar: la numeración queda
        // contigua y el artefacto refleja el hueco real, no un corrimiento.
        page_data.extend(still_failing.iter().map(|f| empty_page(f.page)));
        failed_pages = still_failing;
        page_data.sort_by_key(|p| p.page);
    }

    let encoding_repairs = page_data
        .iter()
        .flat_map(|p| p.lines.iter())
        .filter(|l| l.repaired || l.text.contains('Ã') || l.text.contains('\u{FFFD}'))
        .count();

    let total_chars: usize = page_data.iter().map(|p| p.text_chars()).sum();

    Ok(ExtractedDoc {
        pages: page_count,
        page_data,
        encoding_repairs,
        failed_pages,
        // Cifrado con contraseña: sale sin texto y el OCR no puede abrirlo.
        encrypted: encrypted && total_chars == 0,
    })
}

/// Nº de fragmentos: 1 para documentos pequeños, hasta 8 para los grandes.
fn plan_shards(page_count: usize) -> usize {
    /// Páginas mínimas por fragmento. Por debajo de esto, abrir otro handle
    /// cuesta más de lo que ahorra.
    const MIN_PAGES_PER_SHARD: usize = 24;
    if page_count < MIN_PAGES_PER_SHARD * 2 {
        return 1;
    }
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    cpus.clamp(1, 8)
        .min(page_count.div_ceil(MIN_PAGES_PER_SHARD))
}

/// Reparte `0..page_count` en rangos contiguos `[from, to)`.
fn shard_ranges(page_count: usize, shards: usize) -> Vec<(usize, usize)> {
    if page_count == 0 {
        return Vec::new();
    }
    let shards = if shards == 0 {
        plan_shards(page_count)
    } else {
        shards.min(page_count)
    };
    if shards <= 1 {
        return vec![(0, page_count)];
    }
    let per = page_count.div_ceil(shards);
    let mut out = Vec::with_capacity(shards);
    let mut start = 0usize;
    while start < page_count {
        let end = (start + per).min(page_count);
        out.push((start, end));
        start = end;
    }
    out
}

/// Extrae un rango de páginas con un handle exclusivo del fragmento.
/// Las páginas que fallan se omiten aquí y se reintentan en serie después.
fn extract_shard(path: &Path, from: usize, to: usize) -> Vec<PageData> {
    let mut pages = Vec::with_capacity(to - from);
    let Ok(doc) = PdfDocument::open(path) else {
        // Sin handle no hay páginas; el reintento en serie lo intentará de nuevo.
        return pages;
    };
    for i in from..to {
        if let Ok(page) = extract_page(&doc, i) {
            pages.push(page);
        }
    }
    pages
}

/// Reintenta en serie las páginas que faltaron, con un handle nuevo.
/// Devuelve (páginas recuperadas, páginas irrecuperables).
fn retry_pages(path: &Path, missing: &[usize]) -> (Vec<PageData>, Vec<PageFailure>) {
    let mut recovered = Vec::new();
    let mut failures = Vec::new();
    let Ok(doc) = PdfDocument::open(path) else {
        for &page_index in missing {
            failures.push(PageFailure {
                page: page_index + 1,
                error: "no se pudo abrir el PDF para el reintento en serie".to_string(),
            });
        }
        return (recovered, failures);
    };
    for &page_index in missing {
        match extract_page(&doc, page_index) {
            Ok(page) => recovered.push(page),
            Err(e) => failures.push(PageFailure {
                page: page_index + 1,
                error: format!("{:#}", e),
            }),
        }
    }
    (recovered, failures)
}

/// Índices (0-based) de las páginas sin resultado.
fn missing_pages(page_data: &[PageData], page_count: usize) -> Vec<usize> {
    let mut present = vec![false; page_count];
    for p in page_data {
        if p.page >= 1 && p.page <= page_count {
            present[p.page - 1] = true;
        }
    }
    present
        .iter()
        .enumerate()
        .filter(|(_, ok)| !**ok)
        .map(|(i, _)| i)
        .collect()
}

/// Marcador de una página que no se pudo leer: conserva la numeración.
fn empty_page(page: usize) -> PageData {
    PageData {
        page,
        lines: Vec::new(),
        tables: Vec::new(),
        images: 0,
        classifier_kind: "Unreadable".to_string(),
        ocr: false,
        garbled_ratio: 0.0,
        invisible_text_ratio: 0.0,
        text_glyph_count: 0,
    }
}

fn extract_page(doc: &PdfDocument, page_index: usize) -> Result<PageData> {
    // NOTA: extract_text_lines tiene bugs de encoding y espacios en algunos PDFs
    // (p. ej. PDFs generados con Ghostscript); usamos spans + agrupación propia.
    let spans = doc
        .extract_spans(page_index)
        .with_context(|| "extract_spans falló")?;
    let mut out_lines: Vec<LineData> = group_spans(spans);
    // Orden de lectura: arriba → abajo (y desciende) y de izquierda a derecha.
    out_lines.sort_by(|a, b| {
        (b.bbox[1] + b.bbox[3])
            .partial_cmp(&(a.bbox[1] + a.bbox[3]))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.bbox[0].partial_cmp(&b.bbox[0]).unwrap_or(std::cmp::Ordering::Equal))
    });

    let tables = doc
        .extract_tables(page_index)
        .map(|ts| {
            ts.iter()
                .map(|t| TableData {
                    rows: t
                        .rows
                        .iter()
                        .map(|r| r.cells.iter().map(|c| c.text.clone()).collect())
                        .collect(),
                })
                .collect()
        })
        .unwrap_or_default();

    let images = doc
        .page_image_handles(page_index)
        .map(|v| v.len())
        .unwrap_or(0);

    let classification = doc.classify_page(page_index).ok();
    let (classifier_kind, garbled_ratio, invisible_text_ratio, text_glyph_count) =
        match classification {
            Some(c) => (
                format!("{:?}", c.kind),
                c.signals.garbled_ratio,
                c.signals.invisible_text_ratio,
                c.signals.text_glyph_count as u32,
            ),
            None => ("Unknown".to_string(), 0.0, 0.0, 0),
        };

    Ok(PageData {
        page: page_index + 1,
        lines: out_lines,
        tables,
        images,
        classifier_kind,
        ocr: false,
        garbled_ratio,
        invisible_text_ratio,
        text_glyph_count,
    })
}

impl PageData {
    /// ¿Necesita OCR? (texto escaso o nulo + imágenes = página escaneada)
    pub fn needs_ocr(&self) -> bool {
        self.text_chars() < 20 && self.images > 0
    }

    pub fn text_chars(&self) -> usize {
        self.lines.iter().map(|l| l.text.chars().count()).sum()
    }
}

/// Fusiona líneas OCR en la página (las ordena junto con las nativas).
pub fn merge_ocr_lines(
    doc: &mut ExtractedDoc,
    page_1based: usize,
    lines: Vec<LineData>,
) -> usize {
    let n = lines.len();
    if n == 0 {
        return 0;
    }
    if let Some(page) = doc.page_data.iter_mut().find(|p| p.page == page_1based) {
        page.lines.extend(lines);
        page.ocr = true;
        page.lines.sort_by(|a, b| {
            (b.bbox[1] + b.bbox[3])
                .partial_cmp(&(a.bbox[1] + a.bbox[3]))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(
                    a.bbox[0]
                        .partial_cmp(&b.bbox[0])
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
        });
    }
    n
}

/// Agrupa spans en líneas por proximidad vertical y los ordena por x.
fn group_spans(spans: Vec<pdf_oxide::layout::text_block::TextSpan>) -> Vec<LineData> {
    struct Raw {
        text: String,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        size: f32,
        bold: bool,
    }

    let mut raws: Vec<Raw> = Vec::new();
    for sp in spans {
        let text = sp.text.trim().to_string();
        if text.is_empty() {
            continue;
        }
        let r = &sp.bbox;
        raws.push(Raw {
            text,
            x0: r.x,
            y0: r.y,
            x1: r.x + r.width,
            y1: r.y + r.height,
            size: sp.font_size,
            bold: is_bold(sp.font_weight),
        });
    }
    if raws.is_empty() {
        return Vec::new();
    }

    // Orden de lectura: de arriba hacia abajo; dentro de la misma banda, por x.
    raws.sort_by(|a, b| {
        let ya = (a.y0 + a.y1) * 0.5;
        let yb = (b.y0 + b.y1) * 0.5;
        yb.partial_cmp(&ya)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.x0.partial_cmp(&b.x0).unwrap_or(std::cmp::Ordering::Equal))
    });

    // Clustering por bandas de y.
    let mut groups: Vec<Vec<Raw>> = Vec::new();
    for r in raws {
        let cy = (r.y0 + r.y1) * 0.5;
        if let Some(last) = groups.last_mut() {
            let (lcy, lsize) = match last.first() {
                Some(first) => ((first.y0 + first.y1) * 0.5, first.size),
                None => (cy, r.size),
            };
            let tol = (r.size.max(lsize) * 0.6).max(2.0);
            if (cy - lcy).abs() <= tol {
                last.push(r);
                continue;
            }
        }
        groups.push(vec![r]);
    }

    let mut out = Vec::with_capacity(groups.len());
    for mut g in groups {
        g.sort_by(|a, b| a.x0.partial_cmp(&b.x0).unwrap_or(std::cmp::Ordering::Equal));
        let mut text = String::new();
        let mut prev_x1: Option<f32> = None;
        let mut size = 0.0f32;
        let mut bold = false;
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for r in &g {
            if let Some(px1) = prev_x1 {
                let gap = r.x0 - px1;
                let need_space = gap > r.size * 0.18
                    && !text.ends_with(' ')
                    && !r.text.starts_with(' ');
                if need_space {
                    text.push(' ');
                }
            }
            text.push_str(&r.text);
            prev_x1 = Some(r.x1);
            size = size.max(r.size);
            bold |= r.bold;
            x0 = x0.min(r.x0);
            y0 = y0.min(r.y0);
            x1 = x1.max(r.x1);
            y1 = y1.max(r.y1);
        }
        let text = text.trim().to_string();
        if text.is_empty() {
            continue;
        }
        let (text, repaired) = repair_mojibake(&text);
        out.push(LineData {
            text,
            bbox: [x0, y0, x1, y1],
            font_size: size,
            bold,
            repaired,
            source: None,
        });
    }
    out
}

/// Repara mojibake típico (bytes UTF-8 decodificados como Latin-1) y,
/// cuando el byte se perdió como '?', aplica heurísticas del español.
///
/// Devuelve (texto, reparado). El llamador puede contar reparaciones como
/// anomalía de encoding (pérdida detectable, no silenciosa).
pub fn repair_mojibake(text: &str) -> (String, bool) {
    if !text.contains('Ã') {
        return (text.to_string(), false);
    }

    // 1) Re-decodificar Latin-1 → UTF-8 si todos los char caben en un byte.
    let as_bytes: Option<Vec<u8>> = text
        .chars()
        .map(|c| if (c as u32) <= 0xFF { Some(c as u8) } else { None })
        .collect();
    if let Some(bytes) = as_bytes {
        if let Ok(fixed) = String::from_utf8(bytes) {
            if fixed != text && !fixed.contains('\u{FFFD}') {
                return (fixed, true);
            }
        }
    }

    // 2) Byte perdido ('?'): heurísticas de sufijos frecuentes en español.
    let mut s = text.to_string();
    for (pat, rep) in [
        ("CIÃ?N", "CIÓN"),
        ("ciÃ?n", "ción"),
        ("SIÃ?N", "SIÓN"),
        ("siÃ?n", "sión"),
        ("CIÃ?", "CIÓ"),
        ("ciÃ?", "ció"),
    ] {
        s = s.replace(pat, rep);
    }
    let changed = s != text;
    (s, changed)
}

fn is_bold(weight: pdf_oxide::layout::text_block::FontWeight) -> bool {
    matches!(
        weight,
        pdf_oxide::layout::text_block::FontWeight::Bold
            | pdf_oxide::layout::text_block::FontWeight::SemiBold
            | pdf_oxide::layout::text_block::FontWeight::ExtraBold
            | pdf_oxide::layout::text_block::FontWeight::Black
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// PDF sintético de `n` páginas. Cada página lleva un marcador único
    /// (`PAGINA <n> UNICA`) para detectar páginas perdidas o desplazadas.
    fn pdf_con_n_paginas(n: usize) -> Vec<u8> {
        let mut bodies: Vec<String> = Vec::with_capacity(3 + n * 2);
        bodies.push("<< /Type /Catalog /Pages 2 0 R >>".to_string());
        let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
        bodies.push(format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            kids.join(" "),
            n
        ));
        bodies.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string());
        for i in 0..n {
            let content = format!("BT /F1 12 Tf 20 100 Td (PAGINA {} UNICA) Tj ET", i + 1);
            bodies.push(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents {} 0 R \
                 /Resources << /Font << /F1 3 0 R >> >> >>",
                5 + 2 * i
            ));
            bodies.push(format!(
                "<< /Length {} >>\nstream\n{}\nendstream",
                content.len(),
                content
            ));
        }

        let mut out: Vec<u8> = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in bodies.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", i + 1, body).as_bytes());
        }
        let xref_off = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n", bodies.len() + 1).as_bytes());
        out.extend_from_slice(b"0000000000 65535 f \n");
        for off in &offsets {
            out.extend_from_slice(format!("{:010} 00000 n \n", off).as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
                bodies.len() + 1,
                xref_off
            )
            .as_bytes(),
        );
        out
    }

    fn texto_de(doc: &ExtractedDoc) -> Vec<String> {
        doc.page_data
            .iter()
            .map(|p| p.lines.iter().map(|l| l.text.clone()).collect::<Vec<_>>().join("|"))
            .collect()
    }

    /// El número de fragmentos es una decisión de rendimiento, nunca de
    /// resultado. Si alguna vez deja de serlo, la paralelización está rota.
    #[test]
    fn resultado_no_depende_del_numero_de_fragmentos() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("many.pdf");
        fs::write(&path, pdf_con_n_paginas(120)).unwrap();

        let reference = extract_document_with_shards(&path, 1).unwrap();
        assert_eq!(reference.pages, 120);
        assert_eq!(reference.page_data.len(), 120);
        assert!(reference.failed_pages.is_empty());
        let ref_texto = texto_de(&reference);

        for shards in [2usize, 3, 7, 8, 16] {
            let got = extract_document_with_shards(&path, shards).unwrap();
            assert_eq!(got.pages, reference.pages, "shards={shards}");
            assert_eq!(got.page_data.len(), reference.page_data.len(), "shards={shards}");
            assert_eq!(texto_de(&got), ref_texto, "el texto cambió con shards={shards}");
        }
    }

    /// Ninguna página se pierde y ninguna queda desplazada: cada página
    /// conserva su marcador. Es el invariante que rompe la carrera de
    /// `pdf_oxide` cuando varias hilos comparten un mismo `PdfDocument`.
    #[test]
    fn ninguna_pagina_se_pierde_ni_se_desplaza() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("many.pdf");
        let n = 120usize;
        fs::write(&path, pdf_con_n_paginas(n)).unwrap();

        let doc = extract_document(&path).unwrap();
        assert_eq!(doc.pages, n);
        assert_eq!(doc.page_data.len(), n, "faltan páginas en el resultado");
        assert!(
            doc.failed_pages.is_empty(),
            "páginas fallidas: {:?}",
            doc.failed_pages
        );

        for (i, page) in doc.page_data.iter().enumerate() {
            assert_eq!(page.page, i + 1, "numeración no contigua");
            let texto = page
                .lines
                .iter()
                .map(|l| l.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            assert!(
                texto.contains(&format!("PAGINA {} UNICA", i + 1)),
                "la página {} no contiene su marcador (contenido: {:?})",
                i + 1,
                texto
            );
        }
    }
}
