# Changelog

Todos los cambios relevantes de `pitk`. Formato basado en
[Keep a Changelog](https://keepachangelog.com/es-ES/1.1.0/) y
[SemVer](https://semver.org/lang/es/).

## [0.5.0] — 2026-10-20

Esta versión es sobre **confiar en lo que el índice dice que leyó**. Tres fallos
podían hacer que un documento se viera completo cuando no lo estaba.

### Corregido

- **Pérdida silenciosa de páginas en PDFs grandes.** La extracción en paralelo
  compartía un único `PdfDocument`, cuyo cache de páginas es mutable. Al competir
  varios hilos, una página podía resolverse a `Null` y volver **vacía sin ningún
  error** — se midieron hasta 5.411 caracteres perdidos en un mismo documento — o
  abortar con `Invalid PDF: Page index N not found by scanning`, que dejaba el
  documento entero sin indexar. Ahora cada fragmento de páginas abre su propio
  handle: la extracción es **determinista, completa y más rápida**
  (656 págs: ~930 ms frente a ~1.530 ms).
- **Pruebas de ausencia inválidas por tabuladores.** El texto de algunos PDFs
  separa palabras con tabuladores en vez de espacios, y la búsqueda no normalizaba
  ese separador: una frase **presente** en la página devolvía 0 resultados. Afecta
  `pitk search --receipt` y `pitk cross`, que son la base de una prueba de
  ausencia.
- **PDFs cifrados mal diagnosticados.** Se reportaban como «sin texto extraíble:
  requieren OCR», cuando el OCR no puede abrir un PDF con contraseña. Ahora se
  declaran como cifrados y el aviso lo dice con esas palabras.

### Añadido

- **Garantía de completitud**: toda página que falle se reintenta en serie con un
  handle nuevo; si tampoco se puede leer, conserva su número de página y queda
  declarada con su motivo. El documento se indexa igual, pero nunca con un hueco
  silencioso.
- `meta.json`: nuevos campos `extraction.failed_pages` (páginas no legibles),
  `extraction.encrypted` (PDF protegido) y `page_stats[].error` (motivo por
  página). Son aditivos: los índices anteriores siguen siendo legibles.
- `scripts/verify_corpus.sh` y `scripts/sweep_pdfs.sh`: verificadores de lectura
  completa y determinismo sobre un corpus real (contrastan páginas contra `qpdf`).

### Verificado

- 22 documentos de auditoría · 2.930 páginas · completo e idéntico en 5 rondas.
- 8 libros técnicos · 2.205 páginas · completo e idéntico en 3 rondas.
- 251 PDFs heterogéneos → 246 documentos (5 son duplicados byte a byte,
  colapsados por el store content-addressed) · idéntico en 3 rondas · 25 cifrados
  declarados correctamente · 0 páginas fallidas.
- Pruebas: **29 unitarias + 21 de integración** (50 en total), incluidos los casos
  que fallaban antes (invariante de fragmentos, página perdida/desplazada, PDF
  cifrado, frases con tabuladores).

## [0.4.0] — 2026-10-19

### Añadido

- `pitk flags`: señales de revisión sobre los ítems de una FEV (aritmética, códigos,
  soporte) como candidatos, nunca veredictos.
- `pitk find` y `pitk when`: búsqueda de términos y de fechas en **todos** los
  documentos del workspace.
- Los recibos de búsqueda sobreviven a `pitk index --force`.
- Lenguaje agnóstico de dominio en la salida del motor.

## [0.3.0] — 2026-10-18

### Añadido

- `pitk dossier`: consolida FEV, cruce, fechas y recibos en `dossier.md` +
  `dossier.json`.
- `pitk tree`: índice navegable del documento, con títulos de página legibles y
  neutralización de encabezados/pies repetidos.
- Guía de instalación e integración con agentes.

## [0.2.0]

### Añadido

- OCR opcional con tesseract para páginas escaneadas.
- Recibos de búsqueda inmutables y `pitk prune`.
- Reoptimización del árbol (`tree_optimize`, `pitk optimize`, `tree.raw.json`).

## [0.1.0]

### Añadido

- Store `.docidx/` content-addressed y aislamiento por workspace
  (`init`, `index`, `info`).
- `pitk fev` (ítems conciliados), `pitk dates`, `pitk search --receipt`.
- `pitk render` (PNG de páginas), `pitk cross` (soporte por ítem) y
  `pitk serve --mcp`.
