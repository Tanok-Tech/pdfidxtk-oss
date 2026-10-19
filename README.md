# pdfidxtk (`pitk`)

Motor de evidencia PDF **agnóstico**: convierte cualquier PDF (contratos,
pólizas, actas, facturas, anexos) en un paquete navegable (árbol, bloques,
tablas, texto por página) que un agente (vía CLI o MCP)
consulta sin cargar el documento completo.

## Requisitos

| Vía de instalación | Qué necesitas |
|---|---|
| **Binario del release** | **Nada.** Descárgalo de [Releases](https://github.com/Tanok-Tech/pdfidxtk-oss/releases), dale permiso de ejecución y ponlo en el `PATH`. |
| **`cargo install` / compilar** | **Rust primero** ([rustup.rs](https://rustup.rs)) + un enlazador: Xcode Command Line Tools en macOS (`xcode-select --install`), Visual Studio Build Tools (workload C++) en Windows, `build-essential` en Linux. |
| **OCR de páginas escaneadas** (opcional) | `tesseract` + paquetes de idioma (`tesseract-lang` en macOS). |

Al terminar, verifica con `pitk --version`.

## Instalación


Sin Rust: descarga el binario de tu sistema desde
[Releases](https://github.com/Tanok-Tech/pdfidxtk-oss/releases) y ponlo en el `PATH`.

Con Rust:

```bash
cargo install --git https://github.com/Tanok-Tech/pdfidxtk-oss.git --locked
```

Desde el código:

```bash
git clone https://github.com/Tanok-Tech/pdfidxtk-oss.git && cd pdfidxtk-oss
cargo install --path . --locked
```

Guía por sistema (macOS, Windows, Linux): [`docs/INSTALACION.md`](docs/INSTALACION.md).

## Dominios

El core no asume dominio: indexa y navega cualquier PDF.

- **`pitk fev`** es un **adaptador opcional** para documentos tabulares de
  ítems/precios (facturas, cuentas, estados de cuenta). No hace falta para
  indexar, navegar ni buscar; solo agrega ítems y reconciliación aritmética
  cuando el documento tiene esa estructura.
- **`pitk cross`** y **`pitk dossier`** cruzan los ítems de un documento contra
  sus documentos de soporte y arman el papel de trabajo. No asumen salud ni
  ningún otro dominio.
- **`pitk flags`** deriva señales de revisión de esos ítems (aritmética, códigos,
  autorizaciones, soporte): candidatos con evidencia, nunca veredictos.
- El resto de las órdenes (`info`, `index`, `tree`, `dates`, `search`, `find`,
  `when`, `render`, `serve`, `prune`, `optimize`) trabajan sobre PDFs en general.
- Caso de uso validado: facturación de salud (ver [Validación](#validación)).

## Garantías de diseño

- **Aislamiento por workspace**: el store `.docidx/` vive dentro de la carpeta
  del caso. No existe lookup global; `doc_id` sólo resuelve en su workspace.
- **Content-addressed**: `doc_id = sha256(contenido)[:12]`. Indexar dos veces el
  mismo PDF es idempotente.
- **Atómico**: se escribe en `docs/.tmp-*` y se publica con `rename`. Nunca se
  sirve un índice a medias.
- **Concurrente**: lock `flock` por documento para indexar; lecturas libres sobre
  archivos inmutables. Varias sesiones pueden trabajar a la vez.
- **Pérdida detectable**: `meta.json` declara por página el modo de extracción,
  caracteres, tablas, imágenes, páginas ilegibles y anomalías de encoding.
- **Solo evidencia**: `pitk` no conoce las reglas de auditoría, no emite
  veredictos ni objeciones y no lee los MD normativos del auditor. Entrega
  ítems, páginas, soportes y ausencias fechadas; la calificación es del auditor.

## Uso

```bash
pitk init --account <ID_CASO> .   # crea .docidx/ y PDFIDXTK-USO.md
pitk index                        # descompone todos los PDFs de la carpeta
pitk info                         # estado del workspace
pitk fev factura.pdf --json       # ítems + reconciliación aritmética
pitk dates lote-1.pdf 2026-09-02  # páginas de ese día (búsqueda binaria)
pitk search lote-1.pdf "vigencia" "anexo" --receipt   # prueba de ausencia
pitk find "vigencia" --receipt            # ausencia en toda la carpeta
pitk when 2026-01..2026-12                # fechas en todos los documentos
pitk cross factura.pdf soporte.pdf anexo.pdf   # soporte de cada ítem
pitk tree lote-1.pdf --depth 2          # mapa del documento (páginas por nodo)
pitk render soporte.pdf --page 1         # PNG para contenido visual
pitk flags factura.pdf --cross            # señales de revisión (aritmética, códigos, soporte)
pitk dossier factura.pdf --docs soporte.pdf anexo.pdf --dates 2026-01..2026-12
                                     # papel de trabajo: MD + JSON del caso
pitk serve --mcp                     # servidor MCP (stdio) para agentes
pitk prune --yes                     # limpiar docs cuyo PDF ya no existe
pitk optimize lote-1.pdf --max-pages 20 # reajustar granularidad del árbol
```

`tree.json` viene optimizado (nodos hermanos fusionados para ≤8 páginas;
`--max-pages 0` lo desactiva); el árbol crudo queda en `tree.raw.json` y
`meta.json` guarda `tree_opt` (nodos antes/después, fusionados).

Las páginas escaneadas (sin capa de texto) se OCR-an con `tesseract` si está
en el PATH (`--ocr auto` por defecto): el texto queda en bloques con
`"source":"ocr"` y el PNG en `renders/`.

Todas aceptan `--json`. Códigos de salida: `0` ok · `1` error · `2` uso · `3` ocupado.

## Artefactos por documento

```
.docidx/docs/<doc_id>/
├── meta.json      integridad y controles por página
├── tree.json      árbol optimizado (fusionado ≤8 págs por nodo)
├── tree.raw.json  árbol crudo antes de fusionar
├── blocks.jsonl   una línea por bloque, con página y bbox
├── tables.jsonl   tablas detectadas (página, filas, celdas)
├── dates.idx      fecha → páginas (ordenado, para búsqueda binaria)
├── fev.json       ítems + reconciliación (cacheado)
├── receipts/      recibos de búsqueda inmutables
├── renders/       PNG de páginas renderizadas
├── cross.json     cruce factura ↔ soportes
├── full.md        texto con marcadores <!-- PAGINA N -->
└── chunks/        bloques de páginas para subagentes
```

## Estado (v0.4.0)

- Órdenes: `init`, `index`, `info`, `fev`, `dates`, `search --receipt`, `cross`,
  `render`, `tree`, `find`, `when`, `flags`, `dossier`, `optimize`, `prune`, `serve --mcp`.
- OCR con tesseract para páginas escaneadas, recibos inmutables y árbol optimizado.
- **47 tests** (26 unit + 20 integración).

## Validación

Probado en auditorías reales; los documentos, cifras y nombres de los casos **no se
publican**. La suite cubre store y concurrencia, `fev`, fechas, búsqueda con recibos,
OCR, `cross`, `dossier`, `flags`, MCP y contrato JSON del CLI.

Metodología de una corrida completa: [`docs/E2E-AUDITORIA.md`](docs/E2E-AUDITORIA.md).
