# Guía E2E — analizar un caso con la CLI

> Metodología reproducible para validar el flujo completo sin depender de un caso
> concreto. Los documentos y cifras de las corridas reales no se publican.

## Preparación

```bash
mkdir -p /tmp/caso-demo && cd /tmp/caso-demo
cp /ruta/a/documento.pdf .
pitk init --account DEMO-001
pitk index
pitk info --json
```

## Secuencia (solo órdenes `pitk`)

| # | Orden | Qué valida |
|---|---|---|
| 1 | `pitk info --json` | inventario y estado de extracción |
| 2 | `pitk index` | idempotencia (segunda corrida: "ya indexado") |
| 3 | `pitk tree doc.pdf --depth 2` | índice navegable del documento |
| 4 | `pitk fev factura.pdf --json` | ítems + reconciliación (suma = total declarado) |
| 5 | `pitk cross factura.pdf soporte1.pdf soporte2.pdf --json` | soporte por ítem |
| 6 | `pitk search soporte1.pdf <término> --receipt --json` | prueba de ausencia reproducible |
| 7 | `pitk dates soporte1.pdf <rango> --json` | páginas por fecha |
| 8 | `pitk render soporte1.pdf --page 1 --json` | PNG para contenido visual |
| 9 | `pitk flags factura.pdf --cross --json` | señales de revisión (candidatos) |
| 10 | `pitk dossier factura.pdf --docs soporte1.pdf soporte2.pdf --dates <rango>` | papel de trabajo |

## Qué comprobar

- La reconciliación de `fev` debe dar diferencia **0** (o explicar por qué no aplica).
- `cross` debe reportar el mismo número de ítems que `fev`.
- Cada ausencia citada debe tener su recibo (`--receipt`).
- `dossier.md` debe contener conciliación, cruce, fechas y recibos.
- `pitk flags` no debe emitir veredictos: solo señales con evidencia.

## Tiempos de referencia (máquina de desarrollo)

- Caso pequeño (3 documentos, decenas de páginas): indexación y consultas en menos de 1 s.
- Documento grande (1.000-1.500 páginas): indexación en decenas de segundos (paralela).

## Límites

`pitk` entrega **evidencia**; no emite veredictos y no lee normativa externa.
Las decisiones son del auditor.
