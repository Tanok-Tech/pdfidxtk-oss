# Integración con agentes — `pitk` (CLI canónico)

> Decisión de producto: **la CLI es la interfaz oficial**. MCP es un adaptador
> opcional y **congelado** (no recibe funciones nuevas). Todo lo que el agente
> necesita está en 8 órdenes + `pitk dossier`.

## 1. Instalación

```bash
cd <ruta-del-repo>
cargo install --path . --locked      # deja el binario en ~/.cargo/bin/pitk
```

Si `pitk: command not found`:

```bash
echo 'export PATH="$HOME/.cargo/bin:$PATH"' >> ~/.zshrc && source ~/.zshrc
pitk --version
```

Verificación de que el binario instalado es el del repo:

```bash
which pitk          # → $HOME/.cargo/bin/pitk
pitk --version      # → pitk 0.4.0
```

## 2. Modelo de uso: un caso = una carpeta

```bash
cd "/ruta/al/caso"
pitk init --account <ID_CASO>     # una sola vez: ancla el caso y crea PDFIDXTK-USO.md
pitk index                        # indexa los PDFs del nivel superior
pitk info                         # ¿qué hay y cómo se extrajo?
```

- El store vive en `.docidx/` **dentro** de la carpeta del caso.
- No existe búsqueda entre carpetas: es aislamiento físico, no una convención.
- Re-auditoría: si cambian los PDFs, `pitk index` reindexa lo nuevo y `pitk prune`
  informa/limpia los docs cuyo PDF ya no está.
- Guardia extra para agentes: `pitk --expect <ID_CASO> ...` falla si el workspace
  pertenece a otro caso.

## 3. Prompt de arranque del agente auditor

El auditor linkea en su prompt: sus 3 MD normativos (REGLAS, MANUAL, PROCEDIMIENTO)
+ el `PDFIDXTK-USO.md` que está en la carpeta del caso. Plantilla:

```text
Eres auditor. Trabajas SOLO en la carpeta <RUTA_CASO>.
Herramienta: `pitk` (CLI). Su guía está en ./PDFIDXTK-USO.md — léela primero.

Reglas de trabajo:
1. Empieza con `pitk info --json` y `pitk index` si hace falta.
2. Cifras de la factura SOLO desde `pitk fev` y solo si reconciliation.ok = true.
3. Soportes SOLO desde `pitk cross` / `pitk search` / `pitk dates`.
4. Toda ausencia que sustentes debe quedar con `pitk search ... --receipt`.
5. Manuscritos o contenido visual: `pitk render --page N` (no adivines).
6. Cierra con `pitk dossier` y adjunta dossier.md como papel de trabajo.
7. `pitk` entrega evidencia; la calificación es tuya y según mis reglas.
   Nunca inventes una cifra que no reconcilie ni cites una página no verificada.
```

## 4. MCP opcional (congelado)

Para clientes que **solo** hablan MCP:

```bash
pitk serve --mcp --dir "/ruta/al/caso"
```

Ejemplo de configuración de cliente (stdio):

```json
{
  "mcpServers": {
    "pitk-caso": {
      "command": "$HOME/.cargo/bin/pitk",
      "args": ["serve", "--mcp", "--dir", "/ruta/al/caso"]
    }
  }
}
```

Tools expuestas (8): `pitk_info`, `pitk_index`, `pitk_fev`, `pitk_dates`,
`pitk_search`, `pitk_cross`, `pitk_optimize`, `pitk_render`.
No incluye `dossier` ni recibos: **si necesitas el flujo completo, usa la CLI**.

## 5. Qué NO hace `pitk` (límites deliberados)

- No lee ni interpreta los MD normativos de la auditoría.
- No emite veredictos ni objeciones: no "marca" ítems como no pertinentes.
- No consulta fuentes externas (tarifarios, registros, sistemas del cliente): si
  hacen falta, se aportan como PDF o quedan en el backlog (`pitk flags`, fase 5).
