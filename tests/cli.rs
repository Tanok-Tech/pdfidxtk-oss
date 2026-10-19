//! Tests de integración del CLI: aislamiento, idempotencia, guardia de cuenta
//! y contrato JSON.
//!
//! Construimos un PDF mínimo válido (con xref correcta) para no depender de
//! fixtures externos.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};

fn pitk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_pitk"))
}

/// PDF mínimo de 1 página con el texto "Hola mundo de prueba".
fn tiny_pdf() -> Vec<u8> {
    let content = "BT /F1 12 Tf 20 100 Td (Hola mundo de prueba) Tj ET";
    let bodies = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 4 0 R \
         /Resources << /Font << /F1 5 0 R >> >> >>"
            .to_string(),
        format!(
            "<< /Length {} >>\nstream\n{}\nendstream",
            content.len(),
            content
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];

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

/// PDF de 1 página con varias líneas de texto (para fixtures tipo FEV).
fn pdf_con_lineas(lines: &[&str]) -> Vec<u8> {
    let mut content = String::new();
    let mut y = 700;
    for ln in lines {
        content.push_str(&format!("BT /F1 9 Tf 20 {y} Td ({ln}) Tj ET\n"));
        y -= 14;
    }
    let bodies = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R \
         /Resources << /Font << /F1 5 0 R >> >> >>"
            .to_string(),
        format!(
            "<< /Length {} >>\nstream\n{}endstream",
            content.len(),
            content
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
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

fn make_workspace(dir: &Path, account: &str) -> std::path::PathBuf {
    fs::create_dir_all(dir).unwrap();
    let pdf = dir.join("cuenta.pdf");
    fs::write(&pdf, tiny_pdf()).unwrap();
    let out = pitk()
        .arg("init")
        .arg("--account")
        .arg(account)
        .arg(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "init falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    dir.to_path_buf()
}

#[test]
fn init_crea_store_y_hoja_de_uso() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("cuenta A");
    let out = pitk()
        .arg("init")
        .arg("--account")
        .arg("CUENTA-DEMO")
        .arg(&ws)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(ws.join(".docidx").join("registry.json").exists());
    assert!(ws.join("PDFIDXTK-USO.md").exists());
    let sheet = fs::read_to_string(ws.join("PDFIDXTK-USO.md")).unwrap();
    assert!(sheet.contains("CUENTA-DEMO"));
    assert!(sheet.contains("pitk index"));
}

#[test]
fn index_es_idempotente() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = make_workspace(&tmp.path().join("ws"), "X1");

    let out = pitk().arg("index").arg(&ws).arg("--json").output().unwrap();
    assert!(
        out.status.success(),
        "index falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let first: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(first["docs"].as_array().unwrap().len(), 1);

    // Segunda corrida: debe saltarse el documento.
    let out = pitk().arg("index").arg(&ws).arg("--json").output().unwrap();
    assert!(out.status.success());
    let second: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(second["docs"].as_array().unwrap().len(), 0);
    assert_eq!(second["skipped"].as_array().unwrap().len(), 1);
}

#[test]
fn guardia_de_cuenta_rechaza_cruce() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = make_workspace(&tmp.path().join("ws"), "CUENTA-A");

    // --expect de otra cuenta: debe fallar (exit 1).
    let out = pitk()
        .arg("index")
        .arg(&ws)
        .arg("--expect")
        .arg("CUENTA-B")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("cuenta equivocada"), "stderr: {err}");
}

#[test]
fn aislamiento_entre_workspaces() {
    let tmp = tempfile::tempdir().unwrap();
    let a = make_workspace(&tmp.path().join("cuenta-a"), "A");
    let b = make_workspace(&tmp.path().join("cuenta-b"), "B");

    assert!(pitk().arg("index").arg(&a).output().unwrap().status.success());

    // info de B no ve nada de A.
    let out = pitk()
        .arg("info")
        .arg(&b)
        .arg("--json")
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(v["account"], "B");
    assert_eq!(v["docs"].as_array().unwrap().len(), 0);

    // Y el store de B no contiene doc alguno.
    let docs_b = b.join(".docidx").join("docs");
    let count = fs::read_dir(&docs_b).unwrap().count();
    assert_eq!(count, 0);
}

#[test]
fn json_info_es_parseable() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = make_workspace(&tmp.path().join("ws"), "CAN-1");
    assert!(pitk().arg("index").arg(&ws).output().unwrap().status.success());

    let out = pitk()
        .arg("info")
        .arg(&ws)
        .arg("--json")
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(v["ok"], true);
    let docs = v["docs"].as_array().unwrap();
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0]["status"], "ready");
    assert_eq!(docs[0]["pages"], 1);
    assert!(docs[0]["total_chars"].as_u64().unwrap() > 0);
}

#[test]
fn no_quedan_tmp_tras_publicar() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = make_workspace(&tmp.path().join("ws"), "X");
    assert!(pitk().arg("index").arg(&ws).output().unwrap().status.success());

    let docs = ws.join(".docidx").join("docs");
    for entry in fs::read_dir(&docs).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().to_string();
        assert!(!name.starts_with(".tmp-"), "quedó temporal: {name}");
        assert!(!name.starts_with(".old-"), "quedó backup: {name}");
    }
}

#[test]
fn init_con_cuenta_distinta_falla_sin_force() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = make_workspace(&tmp.path().join("ws"), "A");
    let out = pitk()
        .arg("init")
        .arg("--account")
        .arg("B")
        .arg(&ws)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("anclado"), "stderr: {err}");
}

#[test]
fn fev_autoindexa_y_no_falla_sin_items() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = make_workspace(&tmp.path().join("ws"), "X");
    let pdf = ws.join("cuenta.pdf");

    // Sin `index` previo: fev debe indexar solo y reportar 0 ítems sin romper.
    let out = pitk().arg("fev").arg(&pdf).arg("--json").output().unwrap();
    assert!(
        out.status.success(),
        "fev falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["items"].as_array().unwrap().len(), 0);
    assert_eq!(v["reconciliation"]["ok"], false);
    assert!(v["warnings"].as_array().unwrap().len() >= 1);

    // fev.json quedó escrito en el doc
    let docs = ws.join(".docidx").join("docs");
    let doc_dir = fs::read_dir(&docs)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert!(doc_dir.join("fev.json").exists());
}

#[test]
fn search_encuentra_y_prueba_ausencia_con_recibo() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = make_workspace(&tmp.path().join("ws"), "X");
    let pdf = ws.join("cuenta.pdf");

    let out = pitk()
        .arg("search")
        .arg(&pdf)
        .arg("mundo")
        .arg("zzz")
        .arg("--receipt")
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "search falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let terms = v["terms"].as_array().unwrap();
    assert_eq!(terms[0]["term"], "mundo");
    assert_eq!(terms[0]["hits"], 1);
    assert_eq!(terms[0]["pages"], serde_json::json!([1]));
    assert_eq!(terms[1]["hits"], 0);
    assert!(terms[1]["pages"].as_array().unwrap().is_empty());

    let receipt = v["receipt"].as_str().unwrap();
    assert!(std::path::Path::new(receipt).exists());
    let r: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(receipt).unwrap()).unwrap();
    assert_eq!(r["query"], serde_json::json!(["mundo", "zzz"]));
    assert!(r["corpus_sha256"].as_str().unwrap().len() == 64);
}

#[test]
fn dates_consulta_valida_y_vacia() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = make_workspace(&tmp.path().join("ws"), "X");
    let pdf = ws.join("cuenta.pdf");

    let out = pitk()
        .arg("dates")
        .arg(&pdf)
        .arg("2026-01-01..2026-12-31")
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "dates falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["days"].as_array().unwrap().len(), 0);
    assert_eq!(v["total_hits"], 0);

    // formato inválido → exit 1
    let out = pitk().arg("dates").arg(&pdf).arg("ayer").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn render_genera_png() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = make_workspace(&tmp.path().join("ws"), "X");
    let pdf = ws.join("cuenta.pdf");

    let out = pitk()
        .arg("render")
        .arg(&pdf)
        .arg("--page")
        .arg("1")
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "render falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let path = v["path"].as_str().unwrap();
    assert!(path.ends_with(".png"));
    let meta = fs::metadata(path).unwrap();
    assert!(meta.len() > 1000, "PNG sospechosamente pequeño");

    // página fuera de rango → exit 1
    let out = pitk()
        .arg("render")
        .arg(&pdf)
        .arg("--page")
        .arg("99")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn cross_sin_items_falla_con_mensaje() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = make_workspace(&tmp.path().join("ws"), "X");
    let pdf = ws.join("cuenta.pdf");

    let out = pitk()
        .arg("cross")
        .arg(&pdf)
        .arg(&pdf)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("no produjo ítems"), "stderr: {err}");
}

#[test]
fn mcp_lista_tools_y_rechaza_rutas_fuera_del_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = make_workspace(&tmp.path().join("ws"), "X");

    let mut child = Command::new(env!("CARGO_BIN_EXE_pitk"))
        .arg("serve")
        .arg("--mcp")
        .arg(&ws)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    {
        let stdin = child.stdin.as_mut().unwrap();
        writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{}}}}"#
        )
        .unwrap();
        writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","id":2,"method":"tools/list"}}"#
        )
        .unwrap();
        writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{{"name":"pitk_fev","arguments":{{"pdf":"../../etc/passwd"}}}}}}"#
        )
        .unwrap();
        stdin.flush().unwrap();
    }

    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);
    let mut lines = Vec::new();
    for _ in 0..3 {
        let mut l = String::new();
        reader.read_line(&mut l).unwrap();
        lines.push(l);
    }
    child.kill().ok();

    assert!(lines[0].contains("serverInfo"), "init: {}", lines[0]);
    assert!(lines[1].contains("pitk_fev"), "tools: {}", lines[1]);
    assert!(lines[1].contains("pitk_render"), "tools: {}", lines[1]);
    assert!(lines[2].contains("isError"), "traversal: {}", lines[2]);
    assert!(
        lines[2].contains("fuera del workspace"),
        "traversal: {}",
        lines[2]
    );
}

#[test]
fn ocr_en_pdf_escaneado() {
    // requiere tesseract en el PATH; si no está, el test se omite
    if Command::new("tesseract").arg("--version").output().is_err() {
        eprintln!("tesseract no disponible; omitido");
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    fs::create_dir_all(&ws).unwrap();
    let pdf = ws.join("cuenta.pdf");
    fs::write(&pdf, tiny_pdf()).unwrap();

    // "aplanar": el texto queda rasterizado → el PDF deja de tener capa de texto
    let doc = pdf_oxide::document::PdfDocument::open(&pdf).unwrap();
    let bytes = pdf_oxide::rendering::flatten_to_images(&doc, 150).unwrap();
    fs::write(&pdf, bytes).unwrap();
    let probe = pdf_oxide::document::PdfDocument::open(&pdf).unwrap();
    let t = probe.extract_text(0).unwrap_or_default();
    assert!(
        t.trim().is_empty(),
        "el fixture aplanado aún tiene texto extraíble: {:?}",
        &t[..t.len().min(80)]
    );

    let out = pitk()
        .arg("init")
        .arg("--account")
        .arg("OCR1")
        .arg(&ws)
        .output()
        .unwrap();
    assert!(out.status.success());

    let out = pitk().arg("index").arg(&ws).arg("--json").output().unwrap();
    assert!(
        out.status.success(),
        "index falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // meta: la página quedó en modo ocr con motor registrado
    let docs = ws.join(".docidx").join("docs");
    let doc_dir = docs.read_dir().unwrap().next().unwrap().unwrap().path();
    let meta: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(doc_dir.join("meta.json")).unwrap()).unwrap();
    assert_eq!(meta["page_stats"][0]["mode"], "ocr", "meta: {meta}");
    assert_eq!(meta["extraction"]["ocr_pages"], serde_json::json!([1]));
    assert!(
        meta["ocr_engine"]
            .as_str()
            .unwrap_or("")
            .contains("tesseract"),
        "falta ocr_engine: {meta}"
    );
    // el PNG queda como artefacto para inspección visual
    assert!(doc_dir.join("renders").join("p0001-300dpi.png").exists());

    // aguas abajo: búsqueda sobre el texto OCR funciona
    let out = pitk()
        .arg("search")
        .arg(&pdf)
        .arg("mundo")
        .arg("--json")
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["terms"][0]["hits"], 1, "OCR no recuperó el texto: {v}");

    // y los bloques marcan la procedencia
    let blocks = fs::read_to_string(doc_dir.join("blocks.jsonl")).unwrap();
    assert!(blocks.contains(r#""source":"ocr""#), "bloques sin source=ocr");
}

#[test]
fn receipts_no_se_sobrescriben_en_reauditoria() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = make_workspace(&tmp.path().join("ws"), "X");
    let pdf = ws.join("cuenta.pdf");

    for _ in 0..2 {
        let out = pitk()
            .arg("search")
            .arg(&pdf)
            .arg("mundo")
            .arg("--receipt")
            .arg("--json")
            .output()
            .unwrap();
        assert!(out.status.success());
    }

    let docs = ws.join(".docidx").join("docs");
    let doc_dir = docs.read_dir().unwrap().next().unwrap().unwrap().path();
    let count = fs::read_dir(doc_dir.join("receipts")).unwrap().count();
    assert_eq!(count, 2, "la segunda búsqueda debía crear un recibo nuevo");
}

#[test]
fn prune_reporta_y_elimina_docs_huerfanos() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = make_workspace(&tmp.path().join("ws"), "X");
    let pdf = ws.join("cuenta.pdf");
    assert!(pitk().arg("index").arg(&ws).output().unwrap().status.success());

    // el PDF desaparece (caso real: auditor corrige/renombra archivos)
    fs::remove_file(&pdf).unwrap();

    // informe: detecta 1 huérfano, no borra nada
    let out = pitk().arg("prune").arg(&ws).arg("--json").output().unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["orphan_docs"].as_array().unwrap().len(), 1);
    assert_eq!(v["applied"], false);
    let doc_dir = ws
        .join(".docidx")
        .join("docs")
        .read_dir()
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert!(doc_dir.exists(), "prune sin --yes no debe borrar");

    // con --yes: elimina
    let out = pitk()
        .arg("prune")
        .arg(&ws)
        .arg("--yes")
        .arg("--json")
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["removed"].as_array().unwrap().len(), 1);
    assert!(!doc_dir.exists());
    let info = pitk().arg("info").arg(&ws).arg("--json").output().unwrap();
    let vi: serde_json::Value = serde_json::from_slice(&info.stdout).unwrap();
    assert_eq!(vi["docs"].as_array().unwrap().len(), 0);
}

#[test]
fn dossier_escribe_md_y_json_con_reconciliacion() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    fs::write(
        ws.join("FEV.pdf"),
        pdf_con_lineas(&[
            "1 992511 SERVICIO DE EJEMPLO ALFA 1 162.741 162.741",
            "2 14932 SERVICIO DE EJEMPLO BETA 1 39.600 39.600",
            "TOTAL 202.341",
        ]),
    )
    .unwrap();
    let out = pitk().arg("init").arg("--account").arg("SYNTH").arg(ws).output().unwrap();
    assert!(out.status.success(), "init: {}", String::from_utf8_lossy(&out.stderr));

    let out = pitk()
        .arg("dossier")
        .arg(ws.join("FEV.pdf"))
        .arg("--dates")
        .arg("2026-09")
        .arg("--json")
        .current_dir(ws)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "dossier falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["schema"], "pitk/dossier/1");
    assert_eq!(v["fev"]["items"], 2, "ítems: {v}");
    assert!(v["cross"].is_null(), "sin --docs no debe haber cruce: {v}");
    assert_eq!(v["dates"][0]["days"], 0);

    let doc_id = v["fev"]["doc_id"].as_str().unwrap();
    let doc_dir = ws.join(".docidx").join("docs").join(doc_id);
    let md = fs::read_to_string(doc_dir.join("dossier.md")).unwrap();
    assert!(md.contains("## 1. Conciliación de la FEV"), "{md}");
    assert!(md.contains("$202.341,00"), "suma formateada: {md}");
    assert!(md.contains("_Sin cruce"), "{md}");
    assert!(doc_dir.join("dossier.json").exists());
}

#[test]
fn tree_muestra_indice_con_titulo_legible() {
    let dir = tempfile::tempdir().unwrap();
    let ws = make_workspace(dir.path(), "SYNTH");

    let out = pitk()
        .arg("tree")
        .arg("cuenta.pdf")
        .arg("--depth")
        .arg("1")
        .arg("--json")
        .current_dir(&ws)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "tree falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let root = &v["nodes"][0];
    assert_eq!(root["level"], 0);
    let child = &root["children"][0];
    assert_eq!(child["page"][0], 1);
    let title = child["title"].as_str().unwrap();
    assert!(
        title.contains("Hola mundo de prueba"),
        "el nodo de página debe usar texto legible, no 'Pagina 1': {title}"
    );

    // profundidad 0: solo la raíz
    let out = pitk()
        .arg("tree")
        .arg("cuenta.pdf")
        .arg("--depth")
        .arg("0")
        .arg("--json")
        .current_dir(&ws)
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["nodes"][0]["children"].is_null() || v["nodes"][0]["children"].as_array().unwrap().is_empty());
}

#[test]
fn flags_escribe_json_y_avisa_sin_autorizaciones() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    fs::write(
        ws.join("factura.pdf"),
        pdf_con_lineas(&[
            "1 992511 SERVICIO DE EJEMPLO ALFA 1 162.741 162.741",
            "2 14932 SERVICIO DE EJEMPLO BETA 1 39.600 39.600",
            "TOTAL 202.341",
        ]),
    )
    .unwrap();
    let out = pitk().arg("init").arg("--account").arg("SYNTH").arg(ws).output().unwrap();
    assert!(out.status.success());

    let out = pitk()
        .arg("flags")
        .arg(ws.join("factura.pdf"))
        .arg("--json")
        .current_dir(ws)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "flags falló: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["schema"], "pitk/flags/1");
    assert_eq!(v["items"], 2);
    // sin columna de autorización declarada: cero señales de autorización
    assert!(v["by_code"]["autorizacion_ausente"].is_null());
    assert!(
        v["warnings"].as_array().unwrap().iter().any(|w| w.as_str().unwrap().contains("autorizaciones")),
        "{v}"
    );
    let doc_id = v["doc_id"].as_str().unwrap();
    assert!(ws.join(".docidx").join("docs").join(doc_id).join("flags.json").exists());

    // --cross sin cruce previo: error accionable
    let out = pitk()
        .arg("flags")
        .arg(ws.join("factura.pdf"))
        .arg("--cross")
        .current_dir(ws)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("pitk cross"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn reindex_preserva_recibos_de_busqueda() {
    let dir = tempfile::tempdir().unwrap();
    let ws = make_workspace(dir.path(), "SYNTH");

    let out = pitk()
        .arg("search")
        .arg("cuenta.pdf")
        .arg("hola")
        .arg("--receipt")
        .arg("--json")
        .current_dir(&ws)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let receipt = v["receipt"].as_str().unwrap().to_string();
    assert!(Path::new(&receipt).exists());
    let before = fs::read_to_string(&receipt).unwrap();

    let out = pitk()
        .arg("index")
        .arg("--force")
        .arg("--json")
        .current_dir(&ws)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

    assert!(
        Path::new(&receipt).exists(),
        "el recibo debe sobrevivir a `index --force`"
    );
    assert_eq!(before, fs::read_to_string(&receipt).unwrap(), "recibo alterado");
}
