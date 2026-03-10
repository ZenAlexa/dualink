#!/usr/bin/env bash
# Sync reference repos for competitive analysis
# Usage: ./scripts/ref-sync.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REF_DIR="$SCRIPT_DIR/../_reference"

REPOS=(
  "deskflow/deskflow"
  "input-leap/input-leap"
  "debauchee/barrier"
  "feschber/lan-mouse"
)

mkdir -p "$REF_DIR"

for repo in "${REPOS[@]}"; do
  name="${repo##*/}"
  dir="$REF_DIR/$name"
  if [ -d "$dir" ]; then
    echo "Updating $name..."
    git -C "$dir" pull --ff-only 2>/dev/null || git -C "$dir" fetch --depth 1
  else
    echo "Cloning $name..."
    git clone --depth 1 "https://github.com/$repo.git" "$dir"
  fi
done

echo "Done. Reference repos in _reference/"
