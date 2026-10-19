//! Extracción de PDF con `pdf_oxide` (puro Rust, sin dependencias externas).
//!
//! Salida por página: líneas de texto con bbox/fuente, tablas con celdas,
//! conteo de imágenes y señales de calidad (garbled / texto invisible).

use anyhow::{Context, Result};
use pdf_oxide::document::PdfDocument;
use pdf_oxide::layout::text_block::TextLine;
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
}

pub fn extract_document(path: &Path) -> Result<ExtractedDoc> {
    let doc = PdfDocument::open(path)
        .with_context(|| format!("pdf_oxide no pudo abrir {}", path.display()))?;
    let pages = doc.page_count().context("no se pudo leer el número de páginas")?;

    let mut page_data: Vec<PageData> = (0..pages)
        .into_par_iter()
        .map(|i| extract_page(&doc, i).with_context(|| format!("página {}", i + 1)))
        .collect::<Result<Vec<_>>>()?;

    page_data.sort_by_key(|p| p.page);
    let encoding_repairs = page_data
        .iter()
        .flat_map(|p| p.lines.iter())
        .filter(|l| l.repaired || l.text.contains('Ã') || l.text.contains('\u{FFFD}'))
        .count();

    Ok(ExtractedDoc {
        pages,
        page_data,
        encoding_repairs,
    })
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
