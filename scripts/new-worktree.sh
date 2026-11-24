#!/usr/bin/env bash
# Create a git worktree for one workstream, next to the main checkout.
#   scripts/new-worktree.sh graph            -> ../mold.worktrees/graph on branch ws/graph
#   scripts/new-worktree.sh graph ws/mol     -> branch from ws/mol instead of main
# Workstream names are listed in docs/workstreams.md.
set -euo pipefail

name="${1:?usage: new-worktree.sh <workstream> [base]}"
base="${2:-main}"
known="spike mol chem geom graph jobs recipe nodes render ui-graph ui-panels app workers-py fixtures"
if [[ " $known " != *" $name "* ]]; then
  echo "Unknown workstream '$name'. Known: $known (see docs/workstreams.md)" >&2
  exit 1
fi

root="$(git rev-parse --show-toplevel)"
dir="$(dirname "$root")/mold.worktrees/$name"
branch="ws/$name"

if git -C "$root" show-ref --verify --quiet "refs/heads/$branch"; then
  git -C "$root" worktree add "$dir" "$branch"
else
  git -C "$root" worktree add -b "$branch" "$dir" "$base"
fi
echo
echo "Worktree ready: $dir  (branch $branch)"
echo "Brief: docs/workstreams.md#$name, then the CLAUDE.md of the crate(s) you own."
