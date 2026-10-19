//! Store local del workspace: `.docidx/`.
//!
//! Garantías de diseño:
//! - **Aislamiento**: el store vive DENTRO de la carpeta de la cuenta. No existe
//!   lookup global: un `doc_id` solo resuelve si está indexado en ESTE workspace.
//! - **Content-addressed**: `doc_id = sha256(contenido)[:12]`, idempotente.
//! - **Atómico**: se escribe en `docs/.tmp-<id>-<pid>/` y se publica con `rename`.
//!   Un lector ve el índice viejo completo o el nuevo completo, jamás a medias.
//! - **Concurrencia**: lock por documento (`flock`) para indexar; lectores sin lock
//!   sobre archivos inmutables una vez `ready`.

use anyhow::{bail, Context, Result};
use fs2::FileExt;
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::model::{Registry, RegistryDoc, SCHEMA_REGISTRY};

pub const TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const STORE_DIR: &str = ".docidx";

/// Error de recurso ocupado (otra sesión está indexando el mismo doc).
#[derive(Debug)]
pub struct Busy(pub String);

impl fmt::Display for Busy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for Busy {}

pub struct Store {
    pub workspace: PathBuf,
}

impl Store {
    // ── rutas ────────────────────────────────────────────────────────────────

    pub fn store_dir(&self) -> PathBuf {
        self.workspace.join(STORE_DIR)
    }

    pub fn docs_dir(&self) -> PathBuf {
        self.store_dir().join("docs")
    }

    pub fn locks_dir(&self) -> PathBuf {
        self.store_dir().join("locks")
    }

    pub fn registry_path(&self) -> PathBuf {
        self.store_dir().join("registry.json")
    }

    pub fn doc_dir(&self, doc_id: &str) -> PathBuf {
        self.docs_dir().join(doc_id)
    }

    pub fn usage_sheet_path(&self) -> PathBuf {
        self.workspace.join("PDFIDXTK-USO.md")
    }

    // ── apertura / creación ──────────────────────────────────────────────────

    /// Abre un workspace ya inicializado.
    pub fn open(workspace: &Path) -> Result<Store> {
        let ws = workspace
            .canonicalize()
            .with_context(|| format!("no existe el directorio: {}", workspace.display()))?;
        if !ws.is_dir() {
            bail!("no es un directorio: {}", ws.display());
        }
        let store = Store { workspace: ws };
        if !store.registry_path().exists() {
            bail!(
                "workspace sin inicializar: {} — corre: pitk init --account <CUENTA> {}",
                store.store_dir().display(),
                store.workspace.display()
            );
        }
        Ok(store)
    }

    /// Inicializa el workspace. Idempotente: si ya existe y la cuenta coincide,
    /// no falla (solo refresca la hoja de uso).
    pub fn init(workspace: &Path, account: Option<&str>, force: bool) -> Result<(Store, bool)> {
        fs::create_dir_all(workspace)
            .with_context(|| format!("no se pudo crear {}", workspace.display()))?;
        let ws = workspace
            .canonicalize()
            .with_context(|| format!("no se pudo resolver {}", workspace.display()))?;
        let store = Store { workspace: ws };

        fs::create_dir_all(store.docs_dir())?;
        fs::create_dir_all(store.locks_dir())?;

        let mut created = false;
        if store.registry_path().exists() {
            let reg = store.load_registry()?;
            match (&reg.account, account) {
                (Some(existing), Some(new)) if existing != new && !force => {
                    bail!(
                        "este workspace ya está anclado a la cuenta '{}' (pediste '{}'). \
                         Rechazado para evitar cruce de cuentas. Usa --force solo si estás seguro.",
                        existing,
                        new
                    );
                }
                _ => {}
            }
            if force && reg.account.as_deref() != account {
                let _lock = store.lock_registry()?;
                let mut reg2 = store.load_registry()?;
                reg2.account = account.map(|s| s.to_string());
                store.write_registry_atomic(&reg2)?;
            }
        } else {
            let reg = Registry {
                schema: SCHEMA_REGISTRY.to_string(),
                account: account.map(|s| s.to_string()),
                created_at: now_iso(),
                tool_version: TOOL_VERSION.to_string(),
                docs: Default::default(),
            };
            store.write_registry_atomic(&reg)?;
            created = true;
        }

        Ok((store, created))
    }

    // ── registry ─────────────────────────────────────────────────────────────

    pub fn load_registry(&self) -> Result<Registry> {
        let raw = fs::read_to_string(self.registry_path())
            .with_context(|| format!("no se pudo leer {}", self.registry_path().display()))?;
        let reg: Registry = serde_json::from_str(&raw).context("registry.json corrupto")?;
        Ok(reg)
    }

    /// Lock exclusivo sobre el registry (se libera al soltar el File).
    pub fn lock_registry(&self) -> Result<File> {
        let path = self.store_dir().join("registry.lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .with_context(|| format!("no se pudo abrir {}", path.display()))?;
        file.lock_exclusive()
            .context("no se pudo bloquear registry.lock")?;
        Ok(file)
    }

    pub fn write_registry_atomic(&self, reg: &Registry) -> Result<()> {
        let tmp = self
            .store_dir()
            .join(format!("registry.json.tmp.{}", std::process::id()));
        let json = serde_json::to_string_pretty(reg)?;
        {
            let mut f = File::create(&tmp)?;
            f.write_all(json.as_bytes())?;
            f.write_all(b"\n")?;
            f.sync_all().ok();
        }
        fs::rename(&tmp, self.registry_path())?;
        Ok(())
    }

    // ── guardia anti-contaminación (§0.2 del procedimiento del auditor) ──────

    pub fn check_account(&self, expect: Option<&str>) -> Result<()> {
        if let Some(expect) = expect {
            let reg = self.load_registry()?;
            match reg.account.as_deref() {
                Some(actual) if actual == expect => Ok(()),
                Some(actual) => bail!(
                    "cuenta equivocada: este workspace está anclado a '{}', pediste '{}'. \
                     Operación rechazada (aislamiento entre cuentas).",
                    actual,
                    expect
                ),
                None => bail!(
                    "el workspace no tiene cuenta anclada; pediste '{}'. \
                     Re-ancla con: pitk init --account {} --force",
                    expect,
                    expect
                ),
            }
        } else {
            Ok(())
        }
    }

    // ── lock por documento ───────────────────────────────────────────────────

    /// Toma el lock de indexado de un doc. `wait=false` devuelve `Busy` si otra
    /// sesión lo tiene (el llamador decide esperar o reportar `building`).
    pub fn lock_doc(&self, doc_id: &str, wait: bool) -> Result<DocLock> {
        fs::create_dir_all(self.locks_dir())?;
        let path = self.locks_dir().join(format!("{doc_id}.lock"));
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .with_context(|| format!("no se pudo abrir {}", path.display()))?;
        if wait {
            file.lock_exclusive()?;
        } else if file.try_lock_exclusive().is_err() {
            return Err(Busy(format!(
                "doc {} está siendo indexado por otra sesión (espera o usa --wait)",
                doc_id
            ))
            .into());
        }
        Ok(DocLock { file })
    }

    // ── publicación atómica de un doc ────────────────────────────────────────

    pub fn tmp_doc_dir(&self, doc_id: &str) -> PathBuf {
        self.docs_dir()
            .join(format!(".tmp-{}-{}", doc_id, std::process::id()))
    }

    pub fn publish_doc_dir(&self, doc_id: &str, tmp: &Path) -> Result<()> {
        let final_dir = self.doc_dir(doc_id);
        if final_dir.exists() {
            // Reindexación: preservar los artefactos DERIVADOS (recibos,
            // renderizados, fev/cross/flags/dossier, índice de fechas) antes
            // de reemplazar el directorio. El índice solo es dueño de sus
            // propios archivos (meta, árbol, bloques, texto).
            copy_derived_artifacts(&final_dir, tmp);
            // Apartar el viejo y publicar el nuevo.
            let trash = self.docs_dir().join(format!(
                ".old-{}-{}",
                doc_id,
                std::process::id()
            ));
            fs::rename(&final_dir, &trash)?;
            match fs::rename(tmp, &final_dir) {
                Ok(()) => {
                    let _ = fs::remove_dir_all(&trash);
                    Ok(())
                }
                Err(e) => {
                    // restaurar el viejo
                    let _ = fs::rename(&trash, &final_dir);
                    Err(e.into())
                }
            }
        } else {
            fs::rename(tmp, &final_dir)?;
            Ok(())
        }
    }

    pub fn set_registry_doc(&self, doc_id: &str, doc: RegistryDoc) -> Result<()> {
        let _lock = self.lock_registry()?;
        let mut reg = self.load_registry()?;
        reg.docs.insert(doc_id.to_string(), doc);
        self.write_registry_atomic(&reg)?;
        Ok(())
    }
}

#[derive(Debug)]
pub struct DocLock {
    file: File,
}

impl Drop for DocLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}



// ── utilidades ───────────────────────────────────────────────────────────────

pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub fn hash_file(path: &Path) -> Result<(String, u64)> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut total: u64 = 0;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total += n as u64;
    }
    let hex = format!("{:x}", hasher.finalize());
    Ok((hex, total))
}

/// PDFs en el nivel superior del workspace (no recursivo en v1).
pub fn list_pdfs(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        if path
            .extension()
            .map(|e| e.eq_ignore_ascii_case("pdf"))
            .unwrap_or(false)
        {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

/// Evidencia que NO se puede regenerar y debe sobrevivir a una reindexación:
/// recibos de búsqueda (prueba de ausencia) y renderizados PNG.
/// Los artefactos derivados (fev/cross/flags/dossier/dates.idx) se recomputan
/// porque podrían quedar obsoletos si cambia la extracción.
const EVIDENCE_DIRS: &[&str] = &["receipts", "renders"];

fn copy_derived_artifacts(from: &Path, to: &Path) {
    let Ok(rd) = fs::read_dir(from) else {
        return;
    };
    for entry in rd.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy().to_string();
        if !EVIDENCE_DIRS.contains(&name.as_str()) {
            continue;
        }
        let target = to.join(entry.file_name());
        if target.exists() {
            continue; // nunca pisar lo recién generado
        }
        let src = entry.path();
        let _ = if src.is_dir() {
            copy_dir_recursive(&src, &target)
        } else {
            fs::copy(&src, &target).map(|_| ())
        };
    }
}

fn copy_dir_recursive(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)?.flatten() {
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if src.is_dir() {
            copy_dir_recursive(&src, &dst)?;
        } else {
            fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_doc_es_exclusivo() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = Store::init(dir.path(), Some("X"), false).unwrap();
        let _l1 = store.lock_doc("abc123", false).unwrap();
        let err = store.lock_doc("abc123", false).unwrap_err();
        assert!(
            err.downcast_ref::<Busy>().is_some(),
            "el segundo lock debía ser Busy: {err}"
        );
    }

    #[test]
    fn registry_lock_y_write_atomico() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = Store::init(dir.path(), Some("X"), false).unwrap();
        let mut reg = store.load_registry().unwrap();
        reg.docs.insert(
            "deadbeef0000".to_string(),
            crate::model::RegistryDoc {
                file_name: "a.pdf".into(),
                path: "/x/a.pdf".into(),
                sha256: "deadbeef0000aaaa".into(),
                bytes: 10,
                pages: 1,
                status: "ready".into(),
                indexed_at: now_iso(),
            },
        );
        store.write_registry_atomic(&reg).unwrap();
        let back = store.load_registry().unwrap();
        assert!(back.docs.contains_key("deadbeef0000"));
        // sin temporales sueltos
        for entry in fs::read_dir(store.store_dir()).unwrap() {
            let n = entry.unwrap().file_name().to_string_lossy().to_string();
            assert!(!n.contains(".tmp."), "quedó temporal {n}");
        }
    }
}
