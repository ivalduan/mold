#!/usr/bin/env bash
# Enforces the one rule from the engineering plan: no crate below the UI band may
# depend (directly or transitively) on GPUI or on any mold-ui-* / mold-app crate.
# Runs in CI and locally (Linux, macOS, Git Bash on Windows).
set -euo pipefail

HEADLESS_CRATES=(
  mold-mol mold-chem mold-geom mold-render
  mold-graph mold-nodes mold-jobs mold-recipe mold-cli
  mold-mock-cofold
)
FORBIDDEN='^(gpui|gpui-[a-z0-9_-]+|gpui_[a-z0-9_]+|mold-ui-[a-z-]+|mold-app) '

status=0
for crate in "${HEADLESS_CRATES[@]}"; do
  hits=$(cargo tree -p "$crate" -e normal,build --prefix none --all-features 2>/dev/null \
           | grep -E "$FORBIDDEN" | sort -u || true)
  if [[ -n "$hits" ]]; then
    echo "LAYERING VIOLATION: $crate pulls in:"
    echo "$hits" | sed 's/^/    /'
    status=1
  fi
done

if [[ $status -eq 0 ]]; then
  echo "layering ok: ${#HEADLESS_CRATES[@]} headless crates free of GPUI and UI crates"
fi
exit $status
