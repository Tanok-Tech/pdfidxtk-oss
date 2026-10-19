# ROADMAP — pdfidxtk (`pitk`)

> Estado del proyecto y plan de fases. Lee esto + `README.md` al entrar.

## Arranque rápido

1. `cargo test` → debe dar **47/47** (26 unit + 20 integración).
2. `cargo install --path . --locked` → `pitk` en `~/.cargo/bin`.
3. Guía de integración con agentes: [`docs/INTEGRACION-AGENTES.md`](docs/INTEGRACION-AGENTES.md).

## Estado: v0.4.0

| Fase | Qué aportó |
|---|---|
| v1a | store `.docidx/` content-addressed, aislamiento por workspace, `init/index/info`, tests de concurrencia |
| v1b | `fev` (ítems conciliados), `dates` (búsqueda binaria), `search --receipt` |
| v1.5 | `render` (PNG de páginas), `cross` (soporte por ítem), `serve --mcp` |
| v2 | OCR con tesseract, recibos inmutables, `prune` |
| v2b | `tree_optimize` (fusión de nodos), `pitk optimize`, `tree.raw.json` |
| v0.3.0 | `pitk dossier` (papel de trabajo MD/JSON), `pitk tree` (índice navegable), integración CLI |
| v0.4.0 | `pitk flags` (candidatos con evidencia), `pitk find`/`pitk when`, recibos que sobreviven a `index --force` |

**Validación:** cuentas reales de auditoría (documentos y cifras de los casos no se
publican) y **47 tests** (26 unit + 20 integración).

## Comandos (v0.4.0)

```bash
pitk init --account <CASO> .       # crea .docidx/ + PDFIDXTK-USO.md (para el prompt)
pitk index [--ocr auto|off] [--max-pages 8]
pitk info | fev | dates | search --receipt | cross | render --page N
pitk tree --depth N | find | when | flags --cross | dossier --docs --dates
pitk optimize [--max-pages N]      # reajusta granularidad del árbol
pitk prune [--yes]                 # docs cuyo PDF ya no existe
pitk serve --mcp                   # 8 tools MCP (info, index, fev, dates, search, cross, render, optimize)
```

## Fase 3 — Integración con agentes ✅ COMPLETADA (v0.3.0)

> **Decisión de producto:** la **CLI es la interfaz canónica**. MCP queda como
> adaptador opcional **congelado**. Ver `docs/INTEGRACION-AGENTES.md`.

- [x] Instalación: `cargo install --path .` → `pitk` en `~/.cargo/bin` + guía de integración.
- [x] `pitk dossier <factura> [--docs …] [--dates …]` → `dossier.md` + `dossier.json`.
- [x] **Prueba E2E** de un caso completo usando solo las 8 órdenes (`docs/E2E-AUDITORIA.md`).
- [x] `pitk tree <pdf> [--depth N]` con títulos legibles de página (salta encabezados
      y pies repetidos).
- [ ] Hardening MCP: **cancelado por decisión** (MCP congelado opcional).

**DoD:** ✅ un análisis de caso completo ejecutado por el agente SIN comandos fuera
de las 8 órdenes y SIN pedir ayuda humana intermedia.

## Fase 4 — Cobertura de extracción

- [ ] **Tercer medio de extracción** para documentos tabulares hostiles (tablas sin
      bordes): gate = `fev` no reconcilia → sugerir `--engine`. Candidato: Docling
      ([crate](https://crates.io/crates/docling)) o `pdf_oxide::extract_tables` con config.
- [ ] **Fallback geométrico de tablas** para `tables.jsonl`: filas/columnas por
      alineación xy → celdas separadas.
- [ ] **OCR de imágenes incrustadas**: PDFs nativos que contienen escaneos como imagen
      (extract_images → tesseract). Ojo: `has_text_layer` NO sirve como detector
      (devuelve falsos positivos); usar chars<20 && imágenes>0.
- [ ] Calidad OCR: `--psm` por tipo de página y evaluación con diccionarios del dominio.

**DoD:** un documento de otro emisor (no probado) reconcilia, o el gate explica por qué no.

## Fase 5 — Señales de auditoría (siempre "candidatos", nunca veredictos)

> El motor solo entrega evidencia; ningún comando emite juicio.

- [x] `pitk flags`: `cant×VU≠VT`, cantidad no positiva, valor cero, código ausente,
      autorización ausente (si la columna existe) y sin soporte documental (con `--cross`).
- [x] `pitk find` / `pitk when`: búsqueda y fechas en TODA la carpeta, no por documento.
- [ ] Comparación de días de estancia y duplicados como señal explícita.
- [ ] Cruce con registros externos (censo, EHRE, CSV) cuando el usuario aporte el archivo.
- [ ] Export de features por ítem para modelos de scoring (opcional).

## Fase 6 — Ingeniería

- [ ] Rendimiento: perfilar documentos de ~1.500 págs (indexación en decenas de segundos)
      y evitar `extract_tables` en páginas sin líneas.
- [ ] `pitk doctor` (tesseract, versión, integridad del store, hashes).
- [ ] Golden tests con fixture propio (PDF sintético; **nunca** datos personales en el repo).

## Lecciones duras (no repetir)

1. **Nunca validar contra artefactos generados con código anterior** — regenera fixtures
   al tocar la extracción.
2. **Filas de factura fusionadas** (una línea con todo) es el formato real de `pdf_oxide`;
   el parser usa cola numérica saltando `NO/APLICA/COMPARTIDO`. El split por saltos
   horizontales no funciona (13,8 pt de columna vs ~14 pt de espacio).
3. **Cachés con semántica**: cualquier artefacto cacheado lleva `tool_version` y se
   invalida al cambiar.
4. **Rutas relativas**: `Path::parent()` de `foo.pdf` es `""` → helper `parent_dir()`.
5. Parches con `str.replace` de Python fallan **silenciosamente** si el ancla no existe:
   siempre `assert ancla in s`.
6. Pruebas de comando: cubrir rutas relativas además de absolutas; saltar OCR cuando
   `tesseract` no está instalado.

## Convenciones

- Commits atómicos con **qué y por qué**; versión en `Cargo.toml`.
- `cargo test` 47/47 antes de cada commit.
- Datos personales o sensibles: solo en carpetas de casos privados, jamás en el repo.
- El motor **no lee** normativa: `init` solo escribe `PDFIDXTK-USO.md` con las órdenes.
