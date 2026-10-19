//! pitk — pdfidxtk CLI.
//!
//! Motor de evidencia PDF para auditorías: store local content-addressed,
//! aislamiento fuerte por cuenta, sin estado compartido entre sesiones.

mod cross;
mod dossier;
mod extract;
mod flags;
mod fev;
mod index;
mod mcp;
mod model;
mod ocr;
mod query;
mod render;
mod store;
mod tree;
mod usage;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

use crate::model::{InfoDoc, InfoOutput, IndexOutput, InitOutput};
use crate::store::{list_pdfs, now_iso, Store};

#[derive(Parser)]
#[command(
    name = "pitk",
    version,
    about = "pdfidxtk — motor de evidencia PDF para auditorías (store local, aislamiento por cuenta)"
)]
struct Cli {
    /// Salida JSON estable (una sola línea) para agentes
    #[arg(long, global = true)]
    json: bool,

    /// Guardia anti-contaminación: falla si el workspace no está anclado a esta cuenta
    #[arg(long, global = true, value_name = "CUENTA")]
    expect: Option<String>,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Inicializa el workspace: crea .docidx/, ancla la cuenta y genera PDFIDXTK-USO.md
    Init {
        /// Carpeta de la cuenta (por defecto el directorio actual)
        dir: Option<PathBuf>,
        /// Etiqueta del caso, p. ej. CASO-2026-001
        #[arg(long)]
        account: Option<String>,
        /// Re-anclar la cuenta aunque ya exista un anclaje distinto
        #[arg(long)]
        force: bool,
    },
    /// Descompone todos los PDFs de la carpeta en el store
    Index {
        dir: Option<PathBuf>,
        /// Reindexar aunque ya esté listo
        #[arg(long)]
        force: bool,
        /// Esperar si otra sesión está indexando el mismo documento
        #[arg(long)]
        wait: bool,
        /// OCR de páginas escaneadas (requiere tesseract): auto | off
        #[arg(long, default_value = "auto", value_parser = ["auto", "off"])]
        ocr: String,
        /// Nodos del árbol ≤ N páginas (0 = sin optimizar)
        #[arg(long, default_value_t = crate::tree::DEFAULT_MAX_PAGES)]
        max_pages: usize,
    },
    /// Reoptimiza el árbol de un documento indexado (lee tree.raw.json)
    Optimize {
        pdf: PathBuf,
        /// Nodos ≤ N páginas (0 = restaurar árbol crudo)
        #[arg(long, default_value_t = crate::tree::DEFAULT_MAX_PAGES)]
        max_pages: usize,
    },
    /// Señales de revisión sobre los ítems (aritmética, códigos, soporte): candidatos, no veredictos
    Flags {
        /// Ruta del PDF con ítems (factura/cuenta) — indexa si hace falta
        pdf: PathBuf,
        /// Incluir señales de soporte documental (requiere cross.json)
        #[arg(long)]
        cross: bool,
    },
    /// Muestra el índice/árbol del documento (navegación sin leer tree.json)
    Tree {
        /// Ruta del PDF indexado
        pdf: PathBuf,
        /// Niveles por debajo de la raíz a mostrar (0 = solo la raíz)
        #[arg(long, default_value_t = 2)]
        depth: u8,
    },
    /// Estado del workspace y de cada documento indexado
    Info { dir: Option<PathBuf> },
    /// Extrae y reconcilia los ítems de una FEV (requiere indexar antes o lo hace solo)
    Fev {
        /// Ruta del PDF de la factura
        pdf: PathBuf,
        /// Recalcular aunque exista fev.json
        #[arg(long)]
        force: bool,
    },
    /// Páginas que contienen una fecha o rango (búsqueda binaria sobre índice)
    Dates {
        pdf: PathBuf,
        /// 2026-09-02 · 02/09/2026 · 2026-09 · 2026-01-01..2026-12-31
        query: String,
        /// Reconstruir dates.idx desde blocks.jsonl
        #[arg(long)]
        rebuild: bool,
    },
    /// Renderiza una página a PNG (verificación visual de manuscritos)
    Render {
        pdf: PathBuf,
        /// Página 1-based
        #[arg(long)]
        page: usize,
        /// Resolución en DPI (50..600)
        #[arg(long, default_value_t = 300)]
        dpi: u32,
    },
    /// Cruza los ítems de la FEV contra los documentos de soporte de la cuenta
    Cross {
        /// PDF de la factura
        fev: PathBuf,
        /// PDFs de soporte (anexos, comprobantes, contratos…) — uno o varios
        #[arg(required = true)]
        docs: Vec<PathBuf>,
        /// Recalcular aunque exista cross.json
        #[arg(long)]
        force: bool,
    },
    /// Consolida la evidencia de la cuenta (FEV + cruce + fechas + recibos) en MD/JSON
    Dossier {
        /// PDF de la factura
        pdf: PathBuf,
        /// PDFs de soporte para el cruce (anexos, comprobantes, contratos…)
        #[arg(long, num_args = 1..)]
        docs: Vec<PathBuf>,
        /// Consultas de fecha: 2026-09-02 · 2026-09 · 2026-01-01..2026-12-31
        #[arg(long, num_args = 1..)]
        dates: Vec<String>,
        /// Recalcular la FEV aunque exista
        #[arg(long)]
        force: bool,
    },
    /// Reporta (o elimina con --yes) documentos cuyo PDF ya no existe en la carpeta
    Prune {
        dir: Option<PathBuf>,
        /// Eliminar realmente los docs huérfanos (su PDF ya no está)
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        json: bool,
    },
    /// Servidor MCP por stdio (para Antigravity / Harness y otros clientes)
    Serve {
        /// Transporte MCP (stdio)
        #[arg(long)]
        mcp: bool,
        /// Workspace de la cuenta (por defecto el directorio actual)
        dir: Option<PathBuf>,
    },
    /// Busca términos en los bloques del documento (prueba de ausencia con --receipt)
    Search {
        pdf: PathBuf,
        #[arg(required = true)]
        terms: Vec<String>,
        /// Escribe un recibo reproducible con hash del corpus
        #[arg(long)]
        receipt: bool,
    },
    /// Busca términos en TODOS los documentos del workspace (prueba de ausencia con --receipt)
    Find {
        #[arg(required = true)]
        terms: Vec<String>,
        /// Escribe un recibo reproducible por documento
        #[arg(long)]
        receipt: bool,
    },
    /// Páginas con una fecha o rango en TODOS los documentos del workspace
    When {
        /// 2026-09-02 · 02/09/2026 · 2026-09 · 2026-01-01..2026-12-31
        query: String,
    },
}

fn main() {
    let cli = Cli::parse();
    std::process::exit(run(cli));
}

fn run(cli: Cli) -> i32 {
    match dispatch(&cli) {
        Ok(()) => 0,
        Err(e) => {
            if let Some(busy) = e.downcast_ref::<store::Busy>() {
                eprintln!("ocupado: {busy}");
                3
            } else {
                eprintln!("error: {e:#}");
                1
            }
        }
    }
}

fn dispatch(cli: &Cli) -> Result<()> {
    match &cli.cmd {
        Cmd::Init { dir, account, force } => cmd_init(cli, dir.as_deref(), account.as_deref(), *force),
        Cmd::Index {
            dir,
            force,
            wait,
            ocr,
            max_pages,
        } => cmd_index(cli, dir.as_deref(), *force, *wait, ocr.as_str(), *max_pages),
        Cmd::Optimize { pdf, max_pages } => cmd_optimize(cli, pdf, *max_pages),
        Cmd::Flags { pdf, cross } => cmd_flags(cli, pdf, *cross),
        Cmd::Tree { pdf, depth } => cmd_tree(cli, pdf, *depth),
        Cmd::Info { dir } => cmd_info(cli, dir.as_deref()),
        Cmd::Fev { pdf, force } => cmd_fev(cli, pdf, *force),
        Cmd::Dates { pdf, query, rebuild } => cmd_dates(cli, pdf, query, *rebuild),
        Cmd::Render { pdf, page, dpi } => cmd_render(cli, pdf, *page, *dpi),
        Cmd::Cross { fev, docs, force } => cmd_cross(cli, fev, docs, *force),
        Cmd::Dossier {
            pdf,
            docs,
            dates,
            force,
        } => cmd_dossier(cli, pdf, docs, dates, *force),
        Cmd::Serve { mcp, dir } => cmd_serve(cli, *mcp, dir.as_deref()),
        Cmd::Prune { dir, yes, json } => cmd_prune(*yes, *json, dir.as_deref()),
        Cmd::Search {
            pdf,
            terms,
            receipt,
        } => cmd_search(cli, pdf, terms, *receipt),
        Cmd::Find { terms, receipt } => cmd_find(cli, terms, *receipt),
        Cmd::When { query } => cmd_when(cli, query),
    }
}

fn resolve_dir(dir: Option<&Path>) -> Result<PathBuf> {
    match dir {
        Some(d) => Ok(d.to_path_buf()),
        None => Ok(std::env::current_dir()?),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// init
// ─────────────────────────────────────────────────────────────────────────────

fn cmd_init(cli: &Cli, dir: Option<&Path>, account: Option<&str>, force: bool) -> Result<()> {
    let dir = resolve_dir(dir)?;
    let (store, created) = Store::init(&dir, account, force)?;

    let pdfs = list_pdfs(&store.workspace)?;
    let pdf_names: Vec<String> = pdfs
        .iter()
        .map(|p| p.file_name().unwrap_or_default().to_string_lossy().to_string())
        .collect();

    let sheet = usage::render_usage_sheet(
        &store.workspace,
        account,
        &pdf_names,
        &store.store_dir(),
    );
    std::fs::write(store.usage_sheet_path(), &sheet)?;

    if cli.json {
        let out = InitOutput {
            ok: true,
            workspace: store.workspace.display().to_string(),
            store: store.store_dir().display().to_string(),
            account: account.map(|s| s.to_string()),
            created,
            usage_sheet: store.usage_sheet_path().display().to_string(),
            pdfs_detected: pdf_names,
        };
        println!("{}", serde_json::to_string(&out)?);
    } else {
        println!(
            "{} {}",
            if created {
                "Workspace inicializado:"
            } else {
                "Workspace ya inicializado:"
            },
            store.workspace.display()
        );
        println!("  store:    {}", store.store_dir().display());
        println!(
            "  cuenta:   {}",
            account.unwrap_or("(sin anclar — usa --account)")
        );
        println!("  hoja uso: {}", store.usage_sheet_path().display());
        println!(
            "  PDFs:     {} detectados en el nivel superior",
            pdf_names.len()
        );
        for p in &pdf_names {
            println!("    - {}", p);
        }
        println!("\nSiguiente paso: pitk index");
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// index
// ─────────────────────────────────────────────────────────────────────────────

fn cmd_index(
    cli: &Cli,
    dir: Option<&Path>,
    force: bool,
    wait: bool,
    ocr: &str,
    max_pages: usize,
) -> Result<()> {
    let ocr_on = ocr != "off";
    let dir = resolve_dir(dir)?;
    let store = Store::open(&dir)?;
    store.check_account(cli.expect.as_deref())?;

    let pdfs = list_pdfs(&store.workspace)?;
    if pdfs.is_empty() {
        bail!(
            "no hay PDFs en el nivel superior de {}",
            store.workspace.display()
        );
    }

    let mut indexed = Vec::new();
    let mut skipped = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    for path in &pdfs {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if !cli.json {
            eprintln!("→ indexando {}", name);
        }
        match index::index_document(&store, path, force, wait, ocr_on, max_pages) {
            Ok(index::Outcome::Indexed(doc)) => indexed.push(doc),
            Ok(index::Outcome::Skipped(n)) => skipped.push(n),
            Err(e) => {
                // Busy: otra sesión lo tiene → código 3 y se propaga.
                if e.downcast_ref::<store::Busy>().is_some() {
                    return Err(e);
                }
                errors.push(format!("{name}: {e:#}"));
            }
        }
    }

    let had_errors = !errors.is_empty();
    let error_count = errors.len();
    if cli.json {
        let out = IndexOutput {
            ok: !had_errors,
            workspace: store.workspace.display().to_string(),
            account: store.load_registry()?.account,
            docs: indexed,
            skipped,
            errors,
        };
        println!("{}", serde_json::to_string(&out)?);
    } else {
        println!("\nResultado");
        println!("─────────");
        for d in &indexed {
            println!(
                "  ✓ {} [{}] {} págs · {} chars · {} ms",
                d.file_name, d.doc_id, d.pages, d.chars, d.elapsed_ms
            );
            if !d.ocr_required_pages.is_empty() {
                println!(
                    "    ⚠ páginas candidatas a OCR: {:?}",
                    d.ocr_required_pages
                );
            }
        }
        for s in &skipped {
            println!("  = {} (ya indexado; usa --force para reindexar)", s);
        }
        for e in &errors {
            println!("  ✗ {}", e);
        }
        let base = store.store_dir();
        println!("\nArtefactos en {}", base.display());
        if let Some(d) = indexed.first() {
            println!("  ej.: {}/docs/{}/tree.json", base.display(), d.doc_id);
        }
    }

    if had_errors {
        bail!("{} documento(s) fallaron", error_count);
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// info
// ─────────────────────────────────────────────────────────────────────────────

fn cmd_info(cli: &Cli, dir: Option<&Path>) -> Result<()> {
    let dir = resolve_dir(dir)?;
    let store = Store::open(&dir)?;
    store.check_account(cli.expect.as_deref())?;
    let reg = store.load_registry()?;

    let mut docs = Vec::new();
    for (doc_id, d) in &reg.docs {
        let meta_path = store.doc_dir(doc_id).join("meta.json");
        let (native, chars, tables) = match std::fs::read_to_string(&meta_path) {
            Ok(raw) => match serde_json::from_str::<model::MetaFile>(&raw) {
                Ok(m) => (
                    m.extraction.native_pages,
                    m.extraction.total_chars,
                    m.extraction.total_tables,
                ),
                Err(_) => (0, 0, 0),
            },
            Err(_) => (0, 0, 0),
        };
        docs.push(InfoDoc {
            doc_id: doc_id.clone(),
            file_name: d.file_name.clone(),
            status: d.status.clone(),
            pages: d.pages,
            native_pages: native,
            total_chars: chars,
            total_tables: tables,
            indexed_at: d.indexed_at.clone(),
        });
    }
    docs.sort_by(|a, b| a.file_name.cmp(&b.file_name));

    if cli.json {
        let out = InfoOutput {
            ok: true,
            workspace: store.workspace.display().to_string(),
            store: store.store_dir().display().to_string(),
            account: reg.account.clone(),
            docs,
        };
        println!("{}", serde_json::to_string(&out)?);
    } else {
        println!("Workspace: {}", store.workspace.display());
        println!("Store:     {}", store.store_dir().display());
        println!(
            "Cuenta:    {}",
            reg.account.as_deref().unwrap_or("(sin anclar)")
        );
        println!("Docs:      {}", docs.len());
        if docs.is_empty() {
            println!("\n(sin documentos — corre: pitk index)");
        } else {
            println!();
            println!(
                "{:<12} {:<40} {:>5} {:>7} {:>9} {:>7}",
                "DOC_ID", "ARCHIVO", "PÁGS", "NATIVAS", "CHARS", "TABLAS"
            );
            for d in &docs {
                println!(
                    "{:<12} {:<40} {:>5} {:>7} {:>9} {:>7}",
                    d.doc_id,
                    truncate(&d.file_name, 40),
                    d.pages,
                    d.native_pages,
                    d.total_chars,
                    d.total_tables
                );
            }
        }
    }
    let _ = now_iso();
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// fev
// ─────────────────────────────────────────────────────────────────────────────

fn load_blocks(path: &std::path::Path) -> Result<Vec<model::Block>> {
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

fn cmd_fev(cli: &Cli, pdf: &Path, force: bool) -> Result<()> {
    let dir = parent_dir(pdf);
    let store = Store::open(dir)?;
    store.check_account(cli.expect.as_deref())?;

    let (sha, _) = store::hash_file(pdf)?;
    let doc_id: String = sha.chars().take(12).collect();
    let doc_dir = store.doc_dir(&doc_id);
    let fev_path = doc_dir.join("fev.json");
    let cache_valid = fev_path
        .exists()
        && std::fs::read_to_string(&fev_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<fev::FevReport>(&raw).ok())
            .map(|r| r.tool_version == store::TOOL_VERSION)
            .unwrap_or(false);

    if force || !cache_valid {
        if !doc_dir.join("blocks.jsonl").exists() {
            if !cli.json {
                eprintln!("→ documento no indexado; indexando…");
            }
            index::index_document(&store, pdf, false, false, true, tree::DEFAULT_MAX_PAGES)?;
        }
        let blocks = load_blocks(&doc_dir.join("blocks.jsonl"))?;
        let file_name = pdf
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let report = fev::parse_blocks(&doc_id, &file_name, &blocks);
        std::fs::write(&fev_path, serde_json::to_string_pretty(&report)?)?;

        if cli.json {
            println!("{}", serde_json::to_string(&report)?);
            return Ok(());
        }
    } else if cli.json {
        let raw = std::fs::read_to_string(&fev_path)?;
        let v: serde_json::Value = serde_json::from_str(&raw)?;
        println!("{}", serde_json::to_string(&v)?);
        return Ok(());
    }

    let raw = std::fs::read_to_string(&fev_path)?;
    let r: fev::FevReport = serde_json::from_str(&raw)?;

    println!("FEV: {}  [{}]", r.file_name, r.doc_id);
    println!("Modo: {} · ítems: {}", r.mode, r.items.len());
    println!(
        "Suma ítems: {:.2} · Total declarado: {} · Diferencia: {}",
        r.reconciliation.sum_vt,
        r.reconciliation
            .declared_total
            .map(|t| format!("{t:.2}"))
            .unwrap_or_else(|| "—".into()),
        r.reconciliation
            .diff
            .map(|d| format!("{d:+.2}"))
            .unwrap_or_else(|| "—".into())
    );
    println!(
        "Reconciliación: {}",
        if r.reconciliation.ok {
            "✔ OK"
        } else {
            "✗ REVISAR"
        }
    );
    if !r.groups.is_empty() {
        println!("
Grupos:");
        for g in &r.groups {
            println!(
                "  {:<28} {:>4} ítems  suma {:>15.2}  declarado {}",
                g.name,
                g.items,
                g.sum_vt,
                g.declared
                    .map(|d| format!("{d:.2}"))
                    .unwrap_or_else(|| "—".into())
            );
        }
    }
    if !r.reconciliation.missing.is_empty() {
        println!("
Faltantes:");
        for m in &r.reconciliation.missing {
            println!("  - {}", m);
        }
    }
    if !r.reconciliation.duplicates.is_empty() {
        println!("
Duplicados:");
        for d in &r.reconciliation.duplicates {
            println!("  - {}", d);
        }
    }
    if !r.warnings.is_empty() {
        println!("
Advertencias:");
        for w in &r.warnings {
            println!("  ! {}", w);
        }
    }
    println!("
Detalle completo: {}", fev_path.display());
    Ok(())
}

/// Directorio contenedor de un PDF (la ruta relativa "FEV.pdf" tiene parent
/// vacío: en ese caso usamos el directorio actual).
fn parent_dir(pdf: &Path) -> &Path {
    match pdf.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    }
}

fn ensure_indexed(store: &Store, pdf: &Path) -> Result<String> {
    let (sha, _) = store::hash_file(pdf)?;
    let doc_id: String = sha.chars().take(12).collect();
    if !store.doc_dir(&doc_id).join("blocks.jsonl").exists() {
        index::index_document(store, pdf, false, false, true, tree::DEFAULT_MAX_PAGES)?;
    }
    Ok(doc_id)
}

// ─────────────────────────────────────────────────────────────────────────────
// dates
// ─────────────────────────────────────────────────────────────────────────────

fn cmd_dates(cli: &Cli, pdf: &Path, query_str: &str, rebuild: bool) -> Result<()> {
    let dir = parent_dir(pdf);
    let store = Store::open(dir)?;
    store.check_account(cli.expect.as_deref())?;
    let doc_id = ensure_indexed(&store, pdf)?;
    let doc_dir = store.doc_dir(&doc_id);
    let idx_path = doc_dir.join("dates.idx");

    let idx: query::DatesIndex = if idx_path.exists() && !rebuild {
        serde_json::from_str(&std::fs::read_to_string(&idx_path)?)?
    } else {
        let blocks = load_blocks(&doc_dir.join("blocks.jsonl"))?;
        let reg = store.load_registry()?;
        let sha = reg
            .docs
            .get(&doc_id)
            .map(|d| d.sha256.clone())
            .unwrap_or_default();
        let idx = query::build_dates_index(&doc_id, &sha, &blocks);
        std::fs::write(&idx_path, serde_json::to_string_pretty(&idx)?)?;
        idx
    };

    let (start, end) = query::parse_date_query(query_str)?;
    let hits = query::query_dates(&idx, &start, &end);
    let mut pages: Vec<usize> = hits.iter().flat_map(|h| h.pages.clone()).collect();
    pages.sort_unstable();
    pages.dedup();
    let total_hits: usize = hits.iter().map(|h| h.hits).sum();

    if cli.json {
        #[derive(serde::Serialize)]
        struct Out<'a> {
            ok: bool,
            doc_id: &'a str,
            query: QueryOut<'a>,
            days: Vec<&'a query::DateEntry>,
            pages: Vec<usize>,
            total_hits: usize,
        }
        #[derive(serde::Serialize)]
        struct QueryOut<'a> {
            start: &'a str,
            end: &'a str,
        }
        let out = Out {
            ok: true,
            doc_id: &doc_id,
            query: QueryOut {
                start: &start,
                end: &end,
            },
            days: hits,
            pages,
            total_hits,
        };
        println!("{}", serde_json::to_string(&out)?);
        return Ok(());
    }

    println!("Fechas en {} ({} .. {})", doc_id, start, end);
    if hits.is_empty() {
        println!("  (sin coincidencias)");
    } else {
        for h in &hits {
            println!(
                "  {}  {:>3} ocurrencia(s)  páginas {:?}",
                h.date, h.hits, h.pages
            );
        }
        println!(
            "
  {} día(s) · {} ocurrencia(s) · páginas {:?}",
            hits.len(),
            total_hits,
            truncate_vec(&pages, 12)
        );
    }
    Ok(())
}

fn truncate_vec(v: &[usize], n: usize) -> Vec<String> {
    let mut out: Vec<String> = v.iter().take(n).map(|x| x.to_string()).collect();
    if v.len() > n {
        out.push(format!("… (+{})", v.len() - n));
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// render
// ─────────────────────────────────────────────────────────────────────────────

fn cmd_render(cli: &Cli, pdf: &Path, page: usize, dpi: u32) -> Result<()> {
    let dir = parent_dir(pdf);
    let store = Store::open(dir)?;
    store.check_account(cli.expect.as_deref())?;
    let doc_id = ensure_indexed(&store, pdf)?;
    let out_dir = store.doc_dir(&doc_id).join("renders");
    let path = render::render_page(pdf, page, dpi, &out_dir)?;

    if cli.json {
        let out = serde_json::json!({
            "ok": true,
            "doc_id": doc_id,
            "page": page,
            "dpi": dpi,
            "path": path.display().to_string(),
        });
        println!("{}", serde_json::to_string(&out)?);
    } else {
        println!("Renderizado: {}", path.display());
        println!("  doc: {} · página {} · {} dpi", doc_id, page, dpi);
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// flags (señales de revisión)
// ─────────────────────────────────────────────────────────────────────────────

fn cmd_flags(cli: &Cli, pdf: &Path, with_cross: bool) -> Result<()> {
    let dir = parent_dir(pdf);
    let store = Store::open(dir)?;
    store.check_account(cli.expect.as_deref())?;
    let fev = load_or_build_fev(&store, pdf, false)?;
    if fev.items.is_empty() {
        bail!("el documento no produjo ítems; revisa `pitk fev` antes de pedir señales");
    }

    let cross_report = if with_cross {
        let p = store.doc_dir(&fev.doc_id).join("cross.json");
        let found = std::fs::read_to_string(&p)
            .ok()
            .and_then(|raw| serde_json::from_str::<cross::CrossReport>(&raw).ok())
            .filter(|c| c.fev_doc_id == fev.doc_id);
        match found {
            Some(c) => Some(c),
            None => bail!(
                "no hay cruce para este documento; corre `pitk cross <archivo> <soportes...>` primero"
            ),
        }
    } else {
        None
    };

    let report = flags::run(&fev, cross_report.as_ref());
    let out_path = store.doc_dir(&fev.doc_id).join("flags.json");
    std::fs::write(&out_path, serde_json::to_string_pretty(&report)?)?;

    if cli.json {
        println!("{}", serde_json::to_string(&report)?);
        return Ok(());
    }

    println!("Señales en {} · {} ítems", report.file_name, report.items);
    if report.candidates.is_empty() {
        println!("  (sin señales: nada que revisar con las reglas actuales)");
    } else {
        println!(
            "  {} ítem(s) con señales · {}",
            report.flagged_items,
            report
                .by_code
                .iter()
                .map(|(k, v)| format!("{k}: {v}"))
                .collect::<Vec<_>>()
                .join(" · ")
        );
        println!();
        for c in &report.candidates {
            let codes = c
                .flags
                .iter()
                .map(|f| f.code.as_str())
                .collect::<Vec<_>>()
                .join(",");
            println!(
                "  p.{:<4} línea {:<4} {:<14} {:<44} [{}]",
                c.page,
                c.linea,
                c.code1,
                truncate(&c.desc, 44),
                codes
            );
            for f in &c.flags {
                println!("        · {}: {}", f.code, f.detail);
            }
        }
    }
    if !report.warnings.is_empty() {
        println!("\nNotas:");
        for w in &report.warnings {
            println!("  - {w}");
        }
    }
    println!("\nDetalle: {}", out_path.display());
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// tree (navegación del índice)
// ─────────────────────────────────────────────────────────────────────────────

fn cmd_tree(cli: &Cli, pdf: &Path, depth: u8) -> Result<()> {
    let dir = parent_dir(pdf);
    let store = Store::open(dir)?;
    store.check_account(cli.expect.as_deref())?;
    let doc_id = ensure_indexed(&store, pdf)?;
    let tree_path = store.doc_dir(&doc_id).join("tree.json");
    let tree: model::TreeFile = serde_json::from_str(&std::fs::read_to_string(&tree_path)?)?;
    let nodes = tree::outline(&tree, depth);

    if cli.json {
        let out = serde_json::json!({
            "ok": true,
            "doc_id": doc_id,
            "title": tree.title,
            "pages": tree.pages,
            "depth": depth,
            "nodes": nodes,
        });
        println!("{}", serde_json::to_string(&out)?);
        return Ok(());
    }

    println!(
        "{} · {} págs · doc {}",
        tree.title,
        tree.pages,
        doc_id
    );
    fn print_nodes(nodes: &[tree::OutlineNode], indent: usize) {
        for n in nodes {
            let pad = "  ".repeat(indent);
            let mark = if n.children.is_empty() { "·" } else { "▸" };
            println!(
                "{pad}{mark} [p.{}–{}] {}  ({}, nivel {})",
                n.page[0], n.page[1], truncate(&n.title, 90), n.kind, n.level
            );
            print_nodes(&n.children, indent + 1);
        }
    }
    print_nodes(&nodes, 0);
    println!(
        "\n(sin recortar: --depth N · crudo: tree.raw.json)"
    );
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// optimize
// ─────────────────────────────────────────────────────────────────────────────

fn cmd_optimize(cli: &Cli, pdf: &Path, max_pages: usize) -> Result<()> {
    let dir = parent_dir(pdf);
    let store = Store::open(dir)?;
    store.check_account(cli.expect.as_deref())?;
    let doc_id = ensure_indexed(&store, pdf)?;
    let doc_dir = store.doc_dir(&doc_id);

    let raw_path = doc_dir.join("tree.raw.json");
    let tree_path = doc_dir.join("tree.json");
    let base_path = if raw_path.exists() { raw_path } else { tree_path.clone() };
    let base: model::TreeFile =
        serde_json::from_str(&std::fs::read_to_string(&base_path)?)?;

    let (opt, st) = tree::optimize(&base, max_pages);
    std::fs::write(&tree_path, serde_json::to_string_pretty(&opt)?)?;

    // reflejar en meta
    let meta_path = doc_dir.join("meta.json");
    let mut meta: model::MetaFile =
        serde_json::from_str(&std::fs::read_to_string(&meta_path)?)?;
    meta.tree_opt = Some(st.clone());
    std::fs::write(&meta_path, serde_json::to_string_pretty(&meta)?)?;

    if cli.json {
        let out = serde_json::json!({"ok": true, "doc_id": doc_id, "stats": st});
        println!("{}", serde_json::to_string(&out)?);
    } else {
        println!("Árbol optimizado [{}] · max_pages={}", doc_id, st.max_pages);
        println!(
            "  nodos: {} → {} ({} fusionados)",
            st.nodes_before, st.nodes_after, st.merged
        );
        println!(
            "  nodo más grande: {} → {} páginas",
            st.max_span_before, st.max_span_after
        );
        println!("  tree.json actualizado (crudo preservado en tree.raw.json)");
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// prune
// ─────────────────────────────────────────────────────────────────────────────

fn cmd_prune(yes: bool, json_out: bool, dir: Option<&Path>) -> Result<()> {
    let dir = resolve_dir(dir)?;
    let store = Store::open(&dir)?;
    let reg = store.load_registry()?;

    let mut orphans: Vec<(String, String, u64, std::path::PathBuf)> = Vec::new();
    for (doc_id, d) in &reg.docs {
        if !std::path::Path::new(&d.path).exists() {
            orphans.push((
                doc_id.clone(),
                d.file_name.clone(),
                d.bytes,
                store.doc_dir(doc_id),
            ));
        }
    }

    let mut removed: Vec<String> = Vec::new();
    if yes {
        for (doc_id, _, _, doc_dir) in &orphans {
            if doc_dir.exists() {
                std::fs::remove_dir_all(doc_dir)?;
            }
            removed.push(doc_id.clone());
        }
        if !removed.is_empty() {
            let _lock = store.lock_registry()?;
            let mut reg2 = store.load_registry()?;
            for id in &removed {
                reg2.docs.remove(id);
            }
            store.write_registry_atomic(&reg2)?;
        }
    }

    if json_out {
        let out = serde_json::json!({
            "ok": true,
            "orphan_docs": orphans.iter().map(|(id, name, bytes, _)| serde_json::json!({
                "doc_id": id, "file_name": name, "bytes": bytes
            })).collect::<Vec<_>>(),
            "removed": removed,
            "applied": yes
        });
        println!("{}", serde_json::to_string(&out)?);
        return Ok(());
    }

    if orphans.is_empty() {
        println!("Sin docs huérfanos: todos los PDFs del índice siguen en la carpeta.");
        return Ok(());
    }
    println!("Docs cuyo PDF ya no existe en el workspace:");
    for (id, name, bytes, _) in &orphans {
        println!("  {id}  {name}  ({bytes} bytes en el store)");
    }
    if yes {
        println!("\nEliminados: {}", removed.len());
    } else {
        println!(
            "\nNada eliminado todavía. Ejecuta: pitk prune --yes  ({} docs)",
            orphans.len()
        );
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// serve (MCP)
// ─────────────────────────────────────────────────────────────────────────────

fn cmd_serve(cli: &Cli, mcp: bool, dir: Option<&Path>) -> Result<()> {
    if !mcp {
        bail!("solo se soporta el transporte MCP por stdio: usá `pitk serve --mcp`");
    }
    let dir = resolve_dir(dir)?;
    let store = Store::open(&dir)?;
    store.check_account(cli.expect.as_deref())?;
    if !cli.json {
        eprintln!(
            "pitk MCP (stdio) · workspace {} · cuenta {}",
            store.workspace.display(),
            store
                .load_registry()?
                .account
                .as_deref()
                .unwrap_or("(sin anclar)")
        );
    }
    mcp::serve(store.workspace.clone())
}

// ─────────────────────────────────────────────────────────────────────────────
// cross
// ─────────────────────────────────────────────────────────────────────────────

fn load_or_build_fev(store: &Store, pdf: &Path, force: bool) -> Result<fev::FevReport> {
    let doc_id = ensure_indexed(store, pdf)?;
    let fev_path = store.doc_dir(&doc_id).join("fev.json");
    if fev_path.exists() && !force {
        if let Ok(r) = serde_json::from_str::<fev::FevReport>(&std::fs::read_to_string(&fev_path)?)
        {
            if r.tool_version == store::TOOL_VERSION {
                return Ok(r);
            }
        }
    }
    let blocks = load_blocks(&store.doc_dir(&doc_id).join("blocks.jsonl"))?;
    let file_name = pdf
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let report = fev::parse_blocks(&doc_id, &file_name, &blocks);
    std::fs::write(&fev_path, serde_json::to_string_pretty(&report)?)?;
    Ok(report)
}

fn build_cross_report(
    store: &Store,
    fev_pdf: &Path,
    docs: &[PathBuf],
    force: bool,
) -> Result<cross::CrossReport> {
    let report = load_or_build_fev(store, fev_pdf, force)?;
    if report.items.is_empty() {
        bail!(
            "la FEV no produjo ítems (revisa el PDF con `pitk fev`; si es un formato hostil, \
             inspecciona una página con `pitk render --page 1`)"
        );
    }

    let mut loaded: Vec<(cross::DocMeta, Vec<model::Block>)> = Vec::new();
    for doc in docs {
        let doc_id = ensure_indexed(store, doc)?;
        let doc_dir = store.doc_dir(&doc_id);
        let blocks = load_blocks(&doc_dir.join("blocks.jsonl"))?;
        let meta: Option<model::MetaFile> = std::fs::read_to_string(doc_dir.join("meta.json"))
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok());
        loaded.push((
            cross::DocMeta {
                doc_id,
                file_name: doc
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
    let cross = cross::run(
        &report.doc_id,
        &report.file_name,
        &report.items,
        &loaded,
        &synonyms,
    );
    let out_path = store.doc_dir(&report.doc_id).join("cross.json");
    std::fs::write(&out_path, serde_json::to_string_pretty(&cross)?)?;
    Ok(cross)
}

fn cmd_cross(cli: &Cli, fev_pdf: &Path, docs: &[PathBuf], force: bool) -> Result<()> {
    let dir = parent_dir(fev_pdf);
    let store = Store::open(dir)?;
    store.check_account(cli.expect.as_deref())?;

    let report = load_or_build_fev(&store, fev_pdf, false)?;
    let fev_file_name = report.file_name.clone();
    let fev_doc_id = report.doc_id.clone();
    let cross = build_cross_report(&store, fev_pdf, docs, force)?;

    if cli.json {
        println!("{}", serde_json::to_string(&cross)?);
        return Ok(());
    }

    println!(
        "Cruce FEV {} contra {} documento(s)",
        fev_file_name,
        cross.docs.len()
    );
    println!(
        "  ítems: {} · soportados: {} · sin soporte: {} · cobertura: {:.1}%",
        cross.summary.items,
        cross.summary.supported,
        cross.summary.without_support,
        cross.summary.coverage_pct
    );
    println!();
    for it in &cross.items {
        let (mark, detail) = if it.support.is_empty() {
            ("✗", "sin soporte".to_string())
        } else {
            let first = &it.support[0];
            (
                "✓",
                format!(
                    "{} p.{:?} ({})",
                    first.file_name,
                    first.pages.iter().take(4).collect::<Vec<_>>(),
                    first.terms.join(",")
                ),
            )
        };
        println!(
            "  {} {:>3} {:<14} {:<52} {}",
            mark,
            it.linea,
            it.code1,
            truncate(&it.desc, 52),
            detail
        );
    }
    if !cross.warnings.is_empty() {
        println!("\nAdvertencias:");
        for w in &cross.warnings {
            println!("  ! {w}");
        }
    }
    println!(
        "\nDetalle completo: {}",
        store.doc_dir(&fev_doc_id).join("cross.json").display()
    );
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// dossier
// ─────────────────────────────────────────────────────────────────────────────

fn load_dates_index(store: &Store, doc_id: &str) -> Result<query::DatesIndex> {
    let doc_dir = store.doc_dir(doc_id);
    let idx_path = doc_dir.join("dates.idx");
    if idx_path.exists() {
        return Ok(serde_json::from_str(&std::fs::read_to_string(&idx_path)?)?);
    }
    let blocks = load_blocks(&doc_dir.join("blocks.jsonl"))?;
    let reg = store.load_registry()?;
    let sha = reg
        .docs
        .get(doc_id)
        .map(|d| d.sha256.clone())
        .unwrap_or_default();
    let idx = query::build_dates_index(doc_id, &sha, &blocks);
    std::fs::write(&idx_path, serde_json::to_string_pretty(&idx)?)?;
    Ok(idx)
}

fn read_receipts(dir: &Path) -> Vec<dossier::ReceiptRow> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(r) = serde_json::from_str::<query::SearchReceipt>(&raw) else {
            continue;
        };
        out.push(dossier::ReceiptRow {
            file: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            created_at: r.created_at,
            query: r.query.clone(),
            hits: r.results.iter().map(|t| t.hits).sum(),
            corpus_sha256: r.corpus_sha256,
        });
    }
    out.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    out
}

fn cmd_dossier(
    cli: &Cli,
    pdf: &Path,
    docs: &[PathBuf],
    date_queries: &[String],
    force: bool,
) -> Result<()> {
    let dir = parent_dir(pdf);
    let store = Store::open(dir)?;
    store.check_account(cli.expect.as_deref())?;

    let report = load_or_build_fev(&store, pdf, force)?;
    if report.items.is_empty() {
        bail!(
            "la FEV no produjo ítems; revisa el documento con `pitk fev` (y `pitk render --page 1` si es un escaneo)"
        );
    }

    let cross_report = if !docs.is_empty() {
        Some(build_cross_report(&store, pdf, docs, false)?)
    } else {
        let p = store.doc_dir(&report.doc_id).join("cross.json");
        std::fs::read_to_string(&p)
            .ok()
            .and_then(|raw| serde_json::from_str::<cross::CrossReport>(&raw).ok())
            .filter(|c| c.fev_doc_id == report.doc_id)
    };

    let mut dates = Vec::new();
    if !date_queries.is_empty() {
        let idx = load_dates_index(&store, &report.doc_id)?;
        for q in date_queries {
            let (start, end) = query::parse_date_query(q)?;
            let hits: Vec<query::DateEntry> = query::query_dates(&idx, &start, &end)
            .into_iter()
            .cloned()
            .collect();
            let mut pages: Vec<usize> = hits.iter().flat_map(|h| h.pages.clone()).collect();
            pages.sort_unstable();
            pages.dedup();
            dates.push(dossier::DatesRow {
                query: q.clone(),
                start,
                end,
                days: hits.len(),
                hits: hits.iter().map(|h| h.hits).sum(),
                pages,
            });
        }
    }

    let receipts = read_receipts(&store.doc_dir(&report.doc_id).join("receipts"));
    let account = store.load_registry()?.account.clone();
    let d = dossier::build(
        account,
        store::now_iso(),
        &report,
        cross_report.as_ref(),
        dates,
        receipts,
    );

    let doc_dir = store.doc_dir(&report.doc_id);
    let json_path = doc_dir.join("dossier.json");
    let md_path = doc_dir.join("dossier.md");
    std::fs::write(&json_path, serde_json::to_string_pretty(&d)?)?;
    std::fs::write(&md_path, dossier::render_md(&d))?;

    if cli.json {
        println!("{}", serde_json::to_string(&d)?);
        return Ok(());
    }
    println!("Dossier de {}", d.fev.file_name);
    println!(
        "  ítems: {} · suma: ${} · reconcilia: {}",
        d.fev.items,
        format_money(d.fev.sum_vt),
        if d.fev.ok { "sí" } else { "NO" }
    );
    match &d.cross {
        Some(c) => println!(
            "  cruce: {}/{} con soporte ({:.1}%) · {} sin soporte",
            c.supported, c.items, c.coverage_pct, c.without_support
        ),
        None => println!("  cruce: (no ejecutado — pasa --docs para hacerlo)"),
    }
    println!("  fechas: {} consulta(s)", d.dates.len());
    println!("  recibos: {}", d.receipts.len());
    if !d.warnings.is_empty() {
        println!("  advertencias: {}", d.warnings.len());
        for w in d.warnings.iter().take(4) {
            println!("    ! {w}");
        }
    }
    println!("\nMD:   {}", md_path.display());
    println!("JSON: {}", json_path.display());
    Ok(())
}

fn format_money(v: f64) -> String {
    let s = format!("{:.2}", v);
    let (int_part, dec) = s.split_once('.').unwrap_or((s.as_str(), "00"));
    let mut out = String::new();
    for (i, ch) in int_part.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push('.');
        }
        out.push(ch);
    }
    let int: String = out.chars().rev().collect();
    format!("{int},{dec}")
}

// ─────────────────────────────────────────────────────────────────────────────
// search (un documento) y find (toda la cuenta)
// ─────────────────────────────────────────────────────────────────────────────

/// Busca `terms` en los bloques de un doc ya indexado y, si se pide, deja un
/// recibo reproducible. Devuelve (resultados, ruta del recibo).
fn search_doc(
    store: &Store,
    doc_id: &str,
    file_name: &str,
    terms: &[String],
    receipt: bool,
) -> Result<(Vec<query::TermHit>, Option<String>)> {
    let doc_dir = store.doc_dir(doc_id);
    let blocks_path = doc_dir.join("blocks.jsonl");
    let blocks = load_blocks(&blocks_path)?;
    let results = query::search_blocks(&blocks, terms);

    let mut receipt_path: Option<String> = None;
    if receipt {
        let meta: Option<model::MetaFile> = std::fs::read_to_string(doc_dir.join("meta.json"))
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok());
        let corpus = std::fs::read_to_string(&blocks_path)?;
        let corpus_sha = query::hash_text(&corpus);
        let qhash: String = query::hash_text(&terms.join("|"))
            .chars()
            .take(12)
            .collect();
        let r = query::SearchReceipt {
            schema: "pitk/search-receipt/1".to_string(),
            created_at: store::now_iso(),
            doc_id: doc_id.to_string(),
            file_name: file_name.to_string(),
            source_sha256: meta
                .as_ref()
                .map(|m| m.source.sha256.clone())
                .unwrap_or_default(),
            pages: meta.as_ref().map(|m| m.pages).unwrap_or(0),
            blocks: blocks.len(),
            corpus_sha256: corpus_sha,
            query: terms.to_vec(),
            results: results.clone(),
        };
        let dir = doc_dir.join("receipts");
        std::fs::create_dir_all(&dir)?;
        let path = query::unique_receipt_path(&dir, &qhash);
        std::fs::write(&path, serde_json::to_string_pretty(&r)?)?;
        receipt_path = Some(path.display().to_string());
    }
    Ok((results, receipt_path))
}

fn cmd_search(cli: &Cli, pdf: &Path, terms: &[String], receipt: bool) -> Result<()> {
    let dir = parent_dir(pdf);
    let store = Store::open(dir)?;
    store.check_account(cli.expect.as_deref())?;
    let doc_id = ensure_indexed(&store, pdf)?;
    let file_name = pdf
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let (results, receipt_path) = search_doc(&store, &doc_id, &file_name, terms, receipt)?;

    if cli.json {
        #[derive(serde::Serialize)]
        struct Out<'a> {
            ok: bool,
            doc_id: &'a str,
            terms: &'a [query::TermHit],
            #[serde(skip_serializing_if = "Option::is_none")]
            receipt: Option<String>,
        }
        let out = Out {
            ok: true,
            doc_id: &doc_id,
            terms: &results,
            receipt: receipt_path,
        };
        println!("{}", serde_json::to_string(&out)?);
        return Ok(());
    }

    println!("Búsqueda en {} ({} términos)", doc_id, results.len());
    for r in &results {
        let pages = if r.pages.is_empty() {
            "—".to_string()
        } else {
            r.pages
                .iter()
                .take(10)
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join(",")
        };
        println!(
            "  {:<28} {:>4} ocurrencia(s)  páginas {}",
            r.term, r.hits, pages
        );
        if let Some(s) = &r.snippet {
            println!("      {}", truncate(s, 120));
        }
    }
    if let Some(p) = &receipt_path {
        println!("\nRecibo (prueba de términos probados): {}", p);
    }
    Ok(())
}

/// Documentos del workspace listos para consultar, ordenados por nombre.
fn ready_docs(store: &Store) -> Result<Vec<(String, String)>> {
    let reg = store.load_registry()?;
    let mut docs: Vec<(String, String)> = reg
        .docs
        .iter()
        .filter(|(_, d)| d.status == "ready")
        .filter(|(id, _)| store.doc_dir(id).join("blocks.jsonl").exists())
        .map(|(id, d)| (id.clone(), d.file_name.clone()))
        .collect();
    docs.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(docs)
}

fn cmd_find(cli: &Cli, terms: &[String], receipt: bool) -> Result<()> {
    let store = Store::open(&resolve_dir(None)?)?;
    store.check_account(cli.expect.as_deref())?;
    let docs = ready_docs(&store)?;
    if docs.is_empty() {
        bail!(
            "no hay documentos indexados en {}; corre `pitk index` primero",
            store.workspace.display()
        );
    }

    let mut out_docs: Vec<serde_json::Value> = Vec::new();
    let mut totals: std::collections::BTreeMap<String, usize> =
        std::collections::BTreeMap::new();
    let mut rows: Vec<(String, Vec<query::TermHit>, Option<String>)> = Vec::new();

    for (doc_id, file_name) in &docs {
        let (results, receipt_path) = search_doc(&store, doc_id, file_name, terms, receipt)?;
        for r in &results {
            *totals.entry(r.term.clone()).or_default() += r.hits;
        }
        out_docs.push(serde_json::json!({
            "doc_id": doc_id,
            "file_name": file_name,
            "terms": results,
            "receipt": receipt_path,
        }));
        rows.push((file_name.clone(), results, receipt_path));
    }

    if cli.json {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "ok": true,
                "workspace": store.workspace.display().to_string(),
                "terms": terms,
                "totals": totals,
                "docs": out_docs,
            }))?
        );
        return Ok(());
    }

    println!("Búsqueda en {} documento(s)", rows.len());
    for (file_name, results, receipt_path) in &rows {
        let hits: usize = results.iter().map(|r| r.hits).sum();
        println!("\n{}  ({} ocurrencias)", file_name, hits);
        for r in results {
            let pages = r
                .pages
                .iter()
                .take(8)
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join(",");
            println!("  {:<28} {:>4}  páginas {}", r.term, r.hits, pages);
        }
        if let Some(p) = receipt_path {
            println!("  recibo: {}", p);
        }
    }
    println!("\nTotales:");
    for (term, hits) in &totals {
        println!("  {:<28} {}", term, hits);
    }
    Ok(())
}

fn cmd_when(cli: &Cli, query_str: &str) -> Result<()> {
    let store = Store::open(&resolve_dir(None)?)?;
    store.check_account(cli.expect.as_deref())?;
    let docs = ready_docs(&store)?;
    if docs.is_empty() {
        bail!(
            "no hay documentos indexados en {}; corre `pitk index` primero",
            store.workspace.display()
        );
    }

    let (start, end) = query::parse_date_query(query_str)?;
    let mut out_docs: Vec<serde_json::Value> = Vec::new();
    let mut rows: Vec<(String, Vec<query::DateEntry>, Vec<usize>, usize)> = Vec::new();
    let (mut total_days, mut total_hits) = (0usize, 0usize);

    for (doc_id, file_name) in &docs {
        let idx = load_dates_index(&store, doc_id)?;
        let hits: Vec<query::DateEntry> = query::query_dates(&idx, &start, &end)
            .into_iter()
            .cloned()
            .collect();
        let mut pages: Vec<usize> = hits.iter().flat_map(|h| h.pages.clone()).collect();
        pages.sort_unstable();
        pages.dedup();
        let doc_hits: usize = hits.iter().map(|h| h.hits).sum();
        total_days += hits.len();
        total_hits += doc_hits;
        out_docs.push(serde_json::json!({
            "doc_id": doc_id,
            "file_name": file_name,
            "days": hits,
            "pages": pages,
            "total_hits": doc_hits,
        }));
        rows.push((file_name.clone(), hits, pages, doc_hits));
    }

    if cli.json {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "ok": true,
                "workspace": store.workspace.display().to_string(),
                "query": {"start": start, "end": end},
                "totals": {"days": total_days, "hits": total_hits},
                "docs": out_docs,
            }))?
        );
        return Ok(());
    }

    println!("Fechas {} .. {} en {} documento(s)", start, end, rows.len());
    for (file_name, hits, pages, doc_hits) in &rows {
        println!(
            "\n{}  ({} día(s), {} ocurrencia(s), páginas {})",
            file_name,
            hits.len(),
            doc_hits,
            truncate_vec(pages, 10).join(",")
        );
        for h in hits.iter().take(8) {
            println!("  {}  {:>3}  páginas {:?}", h.date, h.hits, h.pages);
        }
        if hits.len() > 8 {
            println!("  … (+{} días)", hits.len() - 8);
        }
    }
    println!("\nTotales: {} día(s), {} ocurrencia(s)", total_days, total_hits);
    Ok(())
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let t: String = s.chars().take(n.saturating_sub(1)).collect();
        format!("{t}…")
    }
}
