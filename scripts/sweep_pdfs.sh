#!/usr/bin/env bash
# Barrido amplio: indexa un directorio lleno de PDFs reales y verifica que
#   1. ningún documento falla,
#   2. el número de páginas indexado coincide con el real (qpdf),
#   3. reindexar produce EXACTAMENTE el mismo contenido (determinismo).
#
# Uso: sweep_pdfs.sh <carpeta-con-pdfs> [rondas]

set -uo pipefail

SRC="${1:?falta la carpeta}"
RONDAS="${2:-2}"
PITK="$(cd "$(dirname "$0")/.." && pwd)/target/release/pitk"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/pitk-sweep.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

mapfile -t pdfs < <(find "$SRC" -iname '*.pdf' -type f | sort)
echo "PDFs encontrados: ${#pdfs[@]}"

for p in "${pdfs[@]}"; do
  cp -p "$p" "$WORK/" 2>/dev/null || true
done

"$PITK" init --account "SWEEP" "$WORK" >/dev/null 2>&1

fallos=0
declare -a huellas=()
for ronda in $(seq 1 "$RONDAS"); do
  if [ "$ronda" -eq 1 ]; then
    "$PITK" index "$WORK" >"$WORK/../log-$ronda.txt" 2>&1
  else
    "$PITK" index "$WORK" --force >"$WORK/../log-$ronda.txt" 2>&1
  fi
  if [ $? -ne 0 ]; then
    echo "✗ ronda $ronda: pitk index devolvió error"
    grep -E "✗" "$WORK/../log-$ronda.txt" | head -20
    fallos=$((fallos + 1))
    break
  fi
  h="$(cat "$WORK"/.docidx/docs/*/full.md 2>/dev/null | shasum -a 256 | cut -d' ' -f1)"
  huellas+=("$h")
  echo "ronda $ronda: huella $h"
done

unicas="$(printf '%s\n' "${huellas[@]}" | sort -u | wc -l | tr -d ' ')"
if [ "$unicas" -gt 1 ]; then
  echo "✗ Extracción NO determinista: $unicas contenidos distintos en $RONDAS rondas"
  fallos=$((fallos + 1))
fi

# Contraste de páginas contra el PDF real
python3 - "$WORK" <<'EOF'
import json, os, subprocess, sys
ws = sys.argv[1]
reg = json.load(open(os.path.join(ws, '.docidx', 'registry.json')))
malos, cifrados = [], []
for doc_id, d in reg['docs'].items():
    meta_path = os.path.join(ws, '.docidx', 'docs', doc_id, 'meta.json')
    meta = json.load(open(meta_path))
    real = subprocess.run(['qpdf', '--show-npages', d['path']],
                          capture_output=True, text=True).stdout.strip()
    if real.isdigit() and int(real) != meta['pages']:
        malos.append((d['file_name'], meta['pages'], real))
    if meta['extraction'].get('encrypted'):
        cifrados.append(d['file_name'])
    if meta['extraction'].get('failed_pages'):
        malos.append((d['file_name'], 'fallidas', meta['extraction']['failed_pages']))

print(f"documentos indexados: {len(reg['docs'])}")
if cifrados:
    print(f"cifrados con contraseña (declarados, no fallan): {cifrados}")
if malos:
    print("DESAJUSTES:")
    for m in malos:
        print("  -", m)
    sys.exit(1)
print("todas las páginas coinciden con el PDF real")
EOF
[ $? -ne 0 ] && fallos=$((fallos + 1))

echo
if [ "$fallos" -gt 0 ]; then
  echo "FALLOS: $fallos"
  exit 1
fi
echo "OK: barrido completo, determinista y sin huecos."
