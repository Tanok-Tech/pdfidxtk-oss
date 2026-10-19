//! OCR de páginas escaneadas con `tesseract` (CLI local, sin red).
//!
//! Si una página no tiene capa de texto (o es casi vacía con imágenes), se
//! renderiza a PNG y se le pasa a tesseract; el TSV de salida se convierte en
//! líneas con bbox en coordenadas PDF (px → pt, eje Y invertido). Esas líneas
//! se fusionan con las nativas y TODO aguas abajo (bloques, fechas, búsqueda,
//! cruce) funciona igual.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::extract::LineData;

#[derive(Debug, Clone)]
pub struct OcrEngine {
    pub version: String,
    pub langs: String,
    pub bin: PathBuf,
}

impl OcrEngine {
    pub fn label(&self) -> String {
        format!("tesseract {} ({})", self.version, self.langs)
    }
}

/// Detecta tesseract en el PATH con soporte de español. `None` si no está.
pub fn detect() -> Option<OcrEngine> {
    let bin = which("tesseract")?;
    let version = Command::new(&bin)
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .next()
                .map(|l| l.trim().to_string())
        })?
        .replace("tesseract ", "")
        .split_whitespace()
        .next()
        .unwrap_or("unknown")
        .to_string();

    let langs_out = Command::new(&bin).arg("--list-langs").output().ok()?;
    let langs_list = String::from_utf8_lossy(&langs_out.stdout).to_lowercase();
    let langs = if langs_list.lines().any(|l| l.trim() == "spa") {
        "spa+eng"
    } else {
        "eng"
    }
    .to_string();

    Some(OcrEngine {
        version,
        langs,
        bin,
    })
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(name);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

/// Ejecuta tesseract sobre un PNG y devuelve líneas ya en coordenadas PDF.
///
/// `page_height_pt` es la altura de la página en puntos (para invertir el eje Y
/// del TSV, que crece hacia abajo).
pub fn ocr_png(
    engine: &OcrEngine,
    png: &Path,
    page_height_pt: f32,
    dpi: u32,
) -> Result<Vec<LineData>> {
    let out = Command::new(&engine.bin)
        .arg(png)
        .arg("stdout")
        .arg("-l")
        .arg(&engine.langs)
        .arg("--psm")
        .arg("3")
        .arg("tsv")
        .output()
        .with_context(|| format!("no se pudo ejecutar {}", engine.bin.display()))?;

    if !out.status.success() {
        bail!(
            "tesseract falló en {}: {}",
            png.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }

    let tsv = String::from_utf8_lossy(&out.stdout);
    Ok(tsv_to_lines(&tsv, page_height_pt, dpi))
}

/// Convierte el TSV de tesseract en líneas con bbox en puntos PDF.
pub fn tsv_to_lines(tsv: &str, page_height_pt: f32, dpi: u32) -> Vec<LineData> {
    let scale = 72.0 / dpi as f32;

    // Agrupar palabras por (block, par, line)
    #[derive(Default)]
    struct Group {
        words: Vec<String>,
        min_left: i64,
        min_top: i64,
        max_right: i64,
        max_bottom: i64,
        max_height: i64,
    }
    use std::collections::HashMap;
    let mut groups: HashMap<(i64, i64, i64), Group> = HashMap::new();

    let mut lines_iter = tsv.lines();
    let header = lines_iter.next().unwrap_or_default();
    if !header.to_lowercase().contains("level") {
        return Vec::new(); // no es TSV esperado
    }

    for line in lines_iter {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 12 {
            continue;
        }
        // level, page, block, par, line, word, left, top, width, height, conf, text
        let level: i64 = cols[0].parse().unwrap_or(0);
        if level != 5 {
            continue; // 5 = palabra
        }
        let conf: f64 = cols[10].parse().unwrap_or(-1.0);
        let text = cols[11].trim();
        if conf < 0.0 || text.is_empty() {
            continue;
        }
        let (block, par, word_line) = (
            cols[2].parse::<i64>().unwrap_or(0),
            cols[3].parse::<i64>().unwrap_or(0),
            cols[4].parse::<i64>().unwrap_or(0),
        );
        let left: i64 = cols[6].parse().unwrap_or(0);
        let top: i64 = cols[7].parse().unwrap_or(0);
        let width: i64 = cols[8].parse().unwrap_or(0);
        let height: i64 = cols[9].parse().unwrap_or(0);

        let g = groups.entry((block, par, word_line)).or_default();
        if g.words.is_empty() {
            g.min_left = left;
            g.min_top = top;
            g.max_right = left + width;
            g.max_bottom = top + height;
        } else {
            g.min_left = g.min_left.min(left);
            g.min_top = g.min_top.min(top);
            g.max_right = g.max_right.max(left + width);
            g.max_bottom = g.max_bottom.max(top + height);
        }
        g.max_height = g.max_height.max(height);
        g.words.push(text.to_string());
    }

    let mut out: Vec<LineData> = groups
        .into_values()
        .map(|g| {
            let text = g.words.join(" ");
            let x0 = g.min_left as f32 * scale;
            let x1 = g.max_right as f32 * scale;
            // px (origen arriba-izquierda) → pt (origen abajo-izquierda)
            let y1 = page_height_pt - g.min_top as f32 * scale;
            let y0 = page_height_pt - g.max_bottom as f32 * scale;
            LineData {
                text,
                bbox: [x0, y0, x1, y1],
                font_size: (g.max_height as f32 * scale).max(4.0),
                bold: false,
                repaired: false,
                source: Some("ocr".to_string()),
            }
        })
        .collect();

    // orden de lectura: arriba → abajo, izquierda → derecha
    out.sort_by(|a, b| {
        (b.bbox[1] + b.bbox[3])
            .partial_cmp(&(a.bbox[1] + a.bbox[3]))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.bbox[0].partial_cmp(&b.bbox[0]).unwrap_or(std::cmp::Ordering::Equal))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tsv_y_coordenadas() {
        let tsv = "\
level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext
1\t1\t0\t0\t0\t0\t0\t0\t1000\t1400\t-1\t
5\t1\t1\t1\t1\t1\t100\t200\t50\t12\t96.5\tFactura
5\t1\t1\t1\t1\t2\t160\t200\t70\t12\t98.0\telectrónica
5\t1\t1\t1\t2\t1\t100\t260\t120\t14\t88.0\tPACIENTE
6\t1\t1\t1\t1\t0\t0\t0\t0\t-1\t
";
        let lines = tsv_to_lines(tsv, 792.0, 300);
        assert_eq!(lines.len(), 2);

        // primera línea (dos palabras agrupadas)
        let l0 = &lines[0]; // tras orden: la de arriba (top=200 < 260)
        assert_eq!(l0.text, "Factura electrónica");
        let scale = 72.0 / 300.0;
        assert!((l0.bbox[0] - 100.0 * scale).abs() < 0.01);
        assert!((l0.bbox[2] - 230.0 * scale).abs() < 0.01);
        // y invertido: arriba en px = alto en pt
        assert!((l0.bbox[3] - (792.0 - 200.0 * scale)).abs() < 0.01);
        assert!((l0.bbox[1] - (792.0 - 212.0 * scale)).abs() < 0.01);
        assert_eq!(l0.source.as_deref(), Some("ocr"));

        let l1 = &lines[1];
        assert_eq!(l1.text, "PACIENTE");
        assert!(l1.bbox[1] < l0.bbox[1], "PACIENTE debe estar más abajo en pt");
    }

    #[test]
    fn detect_sin_tesseract_no_panica() {
        // puede ser Some o None según la máquina; solo debe devolver sin error
        let _ = detect();
    }
}
