#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
case "${1:-}" in
  "") band_setup=true ;;
  --typed) band_setup=false ;;
  *) echo "usage: sh scripts/setup.sh [--typed]" >&2; exit 2 ;;
esac
if "$band_setup"; then
  kit_revision=1e4291e2fa4896b1f603d3d4f0b009c483425da0
  if [ ! -e vexo-open-kit ]; then
    git clone https://github.com/VexoAI-Org/vexo-open-kit.git vexo-open-kit
    git -C vexo-open-kit checkout --detach "$kit_revision"
  fi
  if [ "$(git -C vexo-open-kit rev-parse HEAD)" != "$kit_revision" ]; then
    echo "vexo-open-kit must be at $kit_revision; existing checkout was left unchanged." >&2
    exit 1
  fi
fi
if [ ! -e .venv ]; then
  python3.12 -m venv .venv
fi
.venv/bin/python3 -c 'import sys; assert (3, 9) <= sys.version_info[:2] <= (3, 12), "Use Python 3.12 (supported range 3.9–3.12), not 3.15"'
if "$band_setup"; then
  .venv/bin/python3 -m pip install -r requirements.txt
fi
npm --prefix web ci
