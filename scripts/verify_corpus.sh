#!/usr/bin/env bash
# Verificación de lectura completa sobre un corpus real.
#
# Para cada carpeta-cuenta del corpus:
#   1. indexa el PDF con pitk (varias veces, con --force)
#   2. contrasta el número de páginas indexado contra el real (qpdf)
#   3. comprueba que el resultado es IDÉNTICO entre corridas (mismo sha256 de
#      full.md y mismos chars por página)
#   4. comprueba que no queda ninguna página sin leer (failed_pages vacío)
#
# Uso: verify_corpus.sh <carpeta-corpus> [rondas]

set -uo pipefail

CORPUS="${1:?falta la carpeta del corpus}"
RONDAS="${2:-3}"
PITK="$(cd "$(dirname "$0")/.." && pwd)/target/release/pitk"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/pitk-corpus.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

fallos=0
docs_total=0
paginas_total=0

for cuenta in "$CORPUS"/*/; do
  nombre="$(basename "$cuenta")"
  [ -f "$cuenta" ] && continue

  mapfile -t pdfs < <(find "$cuenta" -maxdepth 1 -name '*.pdf' -o -maxdepth 1 -name '*.PDF' | sort)
  [ ${#pdfs[@]} -eq 0 ] && continue

  ws="$WORK/$nombre"
  mkdir -p "$ws"
  for p in "${pdfs[@]}"; do cp "$p" "$ws/"; done

  "$PITK" init --account "$nombre" "$ws" >/dev/null 2>&1

  echo "── cuenta $nombre (${#pdfs[@]} PDFs)"

  # Cada ronda reindexa y se calcula una huella del contenido extraído.
  # Si dos rondas dan huellas distintas, la extracción no es determinista:
  # el texto de alguna página cambió o se perdió entre corridas.
  declare -a huellas=()
  for ronda in $(seq 1 "$RONDAS"); do
    if [ "$ronda" -gt 1 ]; then
      "$PITK" index "$ws" --force >/dev/null 2>&1
    else
      "$PITK" index "$ws" >/dev/null 2>&1
    fi
    if [ $? -ne 0 ]; then
      echo "  ✗ ronda $ronda: pitk index devolvió error"
      fallos=$((fallos + 1))
      break
    fi
    h="$(cat "$ws"/.docidx/docs/*/full.md 2>/dev/null | shasum -a 256 | cut -d' ' -f1)"
    huellas+=("$h")
  done

  unicas="$(printf '%s\n' "${huellas[@]}" | sort -u | wc -l | tr -d ' ')"
  if [ "$unicas" -gt 1 ]; then
    echo "  ✗ Extracción NO determinista: $unicas contenidos distintos en $RONDAS rondas"
    fallos=$((fallos + 1))
  fi

  # Estado final + verificación contra el PDF real
  for doc_dir in "$ws"/.docidx/docs/*/; do
    [ -d "$doc_dir" ] || continue
    id="$(basename "$doc_dir")"
    meta="$doc_dir/meta.json"
    [ -f "$meta" ] || { echo "  ✗ $id: sin meta.json"; fallos=$((fallos+1)); continue; }

    origen="$(python3 -c "
import json,sys
m=json.load(open('$meta'))
print(m['source']['file_name'], m['pages'], m['extraction']['total_chars'], len(m.get('extraction',{}).get('failed_pages',[])), m['extraction']['total_chars'])
")"
    read -r archivo pgs chars nfail _ <<<"$origen"
    real="$(qpdf --show-npages "$ws/$archivo" 2>/dev/null || echo '?')"

    if [ "$pgs" != "$real" ]; then
      echo "  ✗ $archivo: indexadas $pgs páginas, el PDF tiene $real"
      fallos=$((fallos + 1))
      continue
    fi
    if [ "$nfail" != "0" ]; then
      echo "  ✗ $archivo: $nfail página(s) sin leer"
      fallos=$((fallos + 1))
      continue
    fi

    docs_total=$((docs_total + 1))
    paginas_total=$((paginas_total + pgs))
    printf '  ✓ %-42s %4s págs  %9s chars\n' "$archivo" "$pgs" "$chars"
  done
done

echo
echo "════ $docs_total documentos, $paginas_total páginas leídas ════"
if [ "$fallos" -gt 0 ]; then
  echo "FALLOS: $fallos"
  exit 1
fi
echo "OK: todos los PDFs se leyeron completos y de forma idéntica entre rondas."
