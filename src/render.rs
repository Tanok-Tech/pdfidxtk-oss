//! `pitk render` — renderiza una página a PNG (verificación visual de
//! manuscritos que ni el texto nativo ni el OCR capturan).

use anyhow::{bail, Context, Result};
use pdf_oxide::api::Pdf;
use pdf_oxide::rendering::RenderOptions;
use std::path::{Path, PathBuf};

/// Renderiza `page` (1-based) del PDF a PNG en `out_dir`.
/// Devuelve la ruta del archivo generado.
pub fn render_page(pdf_path: &Path, page: usize, dpi: u32, out_dir: &Path) -> Result<PathBuf> {
    if page == 0 {
        bail!("la página es 1-based (la primera página es 1)");
    }
    if !(50..=600).contains(&dpi) {
        bail!("dpi fuera de rango razonable (50..=600): {dpi}");
    }

    let mut pdf = Pdf::open(pdf_path)
        .with_context(|| format!("no se pudo abrir {}", pdf_path.display()))?;
    let total = pdf
        .page_count()
        .with_context(|| "no se pudo leer el número de páginas")?;
    if page > total {
        bail!("página {page} fuera de rango (el documento tiene {total})");
    }

    let opts = RenderOptions::with_dpi(dpi);
    let image = pdf
        .render_page(page - 1, Some(&opts))
        .with_context(|| format!("falló el render de la página {page}"))?;

    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("no se pudo crear {}", out_dir.display()))?;
    let out = out_dir.join(format!("p{page:04}-{dpi}dpi.png"));
    image
        .save(&out)
        .with_context(|| format!("no se pudo guardar {}", out.display()))?;
    Ok(out)
}
