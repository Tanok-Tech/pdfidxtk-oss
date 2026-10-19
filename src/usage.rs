//! Hoja de uso auto-generada junto al workspace (`PDFIDXTK-USO.md`).
//!
//! El agente la linkea en su prompt igual que sus reglas: le da el
//! descubrimiento de las órdenes SIN cargar esquemas MCP permanentes.

use std::path::Path;

pub fn render_usage_sheet(
    workspace: &Path,
    account: Option<&str>,
    pdfs: &[String],
    store_dir: &Path,
) -> String {
    let account_line = account
        .map(|a| format!("**Cuenta anclada:** `{a}`"))
        .unwrap_or_else(|| {
            "**Cuenta anclada:** (ninguna — corre `pitk init --account <CUENTA> --force`)"
                .to_string()
        });

    let pdfs_list = if pdfs.is_empty() {
        "- (no hay PDFs en el nivel superior de esta carpeta)".to_string()
    } else {
        pdfs.iter()
            .map(|p| format!("- `{p}`"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        r#"# PDFIDXTK (`pitk`) — guía rápida para agentes

Motor de evidencia PDF local y agnóstico al tipo de documento. Descompone los
PDFs de **esta carpeta** (contratos, pólizas, actas, facturas, anexos, soportes)
en un índice navegable (árbol + bloques + páginas) para que no tengas que
leerlos completos.

{account_line}
**Store:** `{store}`
**Workspace:** `{workspace}`

## Regla dura

Trabaja SOLO con los documentos de esta carpeta. El store está anclado a este
workspace; no existe búsqueda global entre workspaces.

## PDFs detectados en esta carpeta

{pdfs}

## Órdenes

Todas aceptan `--json` (salida estable para parsear). Errores van a stderr.

```bash
pitk info --json
# Estado del workspace: docs indexados, páginas, extracción nativa vs OCR.

pitk index
# Descompone todos los PDFs de esta carpeta. Idempotente: si ya está indexado,
# no repite el trabajo (usa --force para reindexar).
#   3 = otro proceso está indexando: espera y reintenta.

pitk info
# Estado legible por humanos.

pitk tree <archivo.pdf> [--depth N] --json
# Índice navegable del documento: nodos con rango de páginas y título. Recorta a
# N niveles bajo la raíz (0 = solo raíz). Úsalo para decidir QUÉ páginas abrir
# antes de leer bloques o renderizar.

pitk optimize <archivo.pdf> [--max-pages N]
# Reoptimiza tree.json: fusiona nodos hermanos para que ninguno cubra >N
# páginas (menos nodos que evaluar). 0 = restaurar árbol crudo. El crudo
# siempre queda en tree.raw.json.

pitk prune [--yes]
# (re-auditoría) Docs cuyo PDF ya no está en la carpeta: informa; --yes los limpia.

pitk dates <archivo.pdf> <fecha|rango> --json
# Páginas que mencionan una fecha, por búsqueda binaria sobre el índice
# cronológico. Formatos: 2026-09-02 · 02/09/2026 · 2026-09 · 2026-01-01..2026-12-31
# Respuesta: days[] con date, pages, hits; pages (unión) y total_hits.

pitk cross <factura.pdf> <doc1.pdf> [doc2.pdf...] --json
# Cruza cada ítem del documento tabular contra los documentos de soporte
# (anexos, actas, contratos...): en qué documento y páginas aparece soporte,
# y qué ítems quedan sin soporte. Escribe cross.json.

pitk render <archivo.pdf> --page N [--dpi 300] --json
# PNG de la página para verificar manuscritos o contenido visual. Devuelve path.

pitk search <archivo.pdf> <término>... [--receipt] --json
# Búsqueda léxica sin acentos sobre los bloques. Con --receipt escribe un
# recibo reproducible (hash del corpus) útil para verificar ausencias
# (por ejemplo, para sustentar una ausencia).
# Respuesta: terms[] con term, hits, pages, snippet; receipt (ruta o null).

pitk find <término>... [--receipt] --json
# Igual que search pero en TODOS los documentos indexados de esta carpeta.
# Úsalo para "¿en qué documento aparece X?" y para ausencias de alcance total.
# Respuesta: totals (término → ocurrencias) + docs[] con terms y receipt.

pitk when <fecha|rango> --json
# Igual que dates pero en TODOS los documentos de la carpeta: qué días
# aparecen en el caso y en qué páginas de cada documento.

pitk fev <factura.pdf> --json
# Extrae los ítems de un documento tabular de ítems/precios (factura, cuenta de
# cobro, estado de cuenta) y RECONCILIA: cantidad × unitario = total por ítem,
# suma de ítems contra el total declarado, faltantes/duplicados por grupo.
# Si el PDF no está indexado, lo indexa primero. Cachea en <doc>/fev.json.
# Campos: items[] con linea, code1, code2, desc, autorizacion, qty, vu, vt, page;
#         reconciliation con items, declared_lines, declared_total, sum_vt, diff,
#         ok, duplicates, missing; mas groups[] y warnings[].

pitk flags <factura.pdf> [--cross] --json
# Señales de revisión sobre los ítems: cant×VU≠VT, cantidad no positiva, valor
# cero, código ausente, autorización ausente (si la columna existe) y sin
# soporte documental (con --cross). Son CANDIDATOS, no veredictos: cada señal
# trae su evidencia (página, línea, valores). Escribe flags.json.

pitk dossier <factura.pdf> [--docs doc1.pdf doc2.pdf...] [--dates <fecha>...] --json
# Consolida en UN solo papel de trabajo: conciliación del documento tabular,
# cruce de ítems vs. soporte, búsquedas de fechas y pruebas de ausencia
# (recibos).
# Escribe dossier.md (para pegar) y dossier.json en la carpeta del documento base.
# Si no pasas --docs reutiliza el cross.json ya calculado.
```

## Contrato de salida

- exit `0` ok · `1` error · `2` uso incorrecto · `3` ocupado (reintenta)
- `--json` imprime un único objeto JSON en stdout
- Los artefactos quedan en `{store}/docs/<doc_id>/`:
  `meta.json` (integridad), `tree.json` (árbol), `blocks.jsonl` (bloques con página),
  `full.md` (texto con marcadores `<!-- PAGINA N -->`) ·
  `tree.raw.json` (árbol sin fusionar)
- `doc_id` = primeros 12 hex del sha256 del PDF

## Flujo de auditoría (orden recomendado)

1. `pitk info --json` → ¿qué PDFs hay y ya están indexados?
2. Si falta: `pitk index` (una vez por workspace; `3` = ocupado, reintenta)
3. `pitk fev factura.pdf --json` → ¿reconcilia el documento? (clave: no citar cifras si no)
4. `pitk cross factura.pdf anexo.pdf soporte.pdf ... --json` → soporte por ítem
5. `pitk tree <doc> --depth 2` → mapa del documento antes de leer páginas
6. `pitk search <doc> <término> --receipt` → pruebas de ausencia reproducibles
   (o `pitk find <término> --receipt` para toda la carpeta)
7. `pitk dates <doc> <fecha|rango>` → páginas de un día/rango
8. `pitk render <doc> --page N` → PNG si hay manuscrito o contenido visual
9. `pitk flags factura.pdf --cross` → señales de revisión (aritmética, códigos, soporte)
10. `pitk dossier factura.pdf --docs ... --dates ...` → papel de trabajo unificado

**Límites del motor (no negociables):** `pitk` entrega EVIDENCIA (ítems, páginas,
soportes, ausencias fechadas). No conoce las reglas de auditoría, no emite
veredictos y nunca debe usarse como fuente única de una decisión.
"#,
        account_line = account_line,
        store = store_dir.display(),
        workspace = workspace.display(),
        pdfs = pdfs_list,
    )
}
