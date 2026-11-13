#!/usr/bin/env bash
# The reference, huc6280-python at the pinned commit (v0.3.0), into external/
# with a venv (its 6502-python dependency comes with it, by its own pin).
# Usage: scripts/fetch_huc6280python.sh [DEST] [SOURCE]
#   SOURCE: a git URL or a local clone (default: GitHub). PYTHON picks the
#   interpreter for the venv (PyPy runs the long rungs several times faster).
set -euo pipefail
COMMIT=bf20330d4ce4e0e126331433bad03f4e4d0e9a2e
ROOT=$(cd "$(dirname "$0")/.." && pwd)
DEST=${1:-"$ROOT/external/huc6280-python"}
SOURCE=${2:-https://github.com/alewman/huc6280-python}
PYTHON=${PYTHON:-"$HOME/.local/opt/pypy3.11-v7.3.20-linux64/bin/pypy3.11"}
command -v "$PYTHON" > /dev/null || PYTHON=python3
if [ ! -d "$DEST/.git" ]; then
    git clone -q "$SOURCE" "$DEST"
fi
git -C "$DEST" fetch -q origin "$COMMIT" 2>/dev/null || true
git -C "$DEST" checkout -q "$COMMIT"
if [ ! -x "$DEST/.venv/bin/python" ]; then
    "$PYTHON" -m venv "$DEST/.venv"
fi
"$DEST/.venv/bin/python" -m pip install -q -e "$DEST"
echo "huc6280-python $(git -C "$DEST" rev-parse --short HEAD) in $DEST"
