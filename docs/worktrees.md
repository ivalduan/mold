# Working in parallel with git worktrees

One workstream = one branch `ws/<name>` = one worktree in `../mold.worktrees/<name>`, with its own agent session. The main checkout stays on `main` and is where contract changes and merges happen.

```
Projects/
  mold/                    main checkout (branch main)
  mold.worktrees/
    graph/                 branch ws/graph
    jobs/                  branch ws/jobs
    spike/                 branch ws/spike
```

## Create
```powershell
.\scripts\new-worktree.ps1 graph            # Windows
```
```sh
scripts/new-worktree.sh graph               # Git Bash / Linux
```
Then open that folder in a new editor window (or `cd` there and start Claude Code). The root `CLAUDE.md` and the crate's `CLAUDE.md` load automatically. Prompt it with: *"You are on workstream `<name>`. Read docs/workstreams.md and your crate's CLAUDE.md, then write the acceptance tests."*

## Rules
1. **Edit only what your workstream owns** ([table](workstreams.md)). Need a change elsewhere? Note it in your PR description or open an issue; don't make it.
2. **Contract changes go to `main` first**, in a small PR, from the main checkout (list in the root `CLAUDE.md`). Every worktree then rebases.
3. **New dependency** = one commit touching root `Cargo.toml` `[workspace.dependencies]` + `THIRD_PARTY.md` only. It's the main conflict hotspot; keep it tiny so rebases are trivial.
4. **Rebase on `main` daily**: `git fetch && git rebase main` (or `origin/main` once there's a remote).
5. **Cargo.lock conflict**: take `main`'s version, then `cargo update -w` / rebuild. Never hand-merge it.
6. **Merge small and often.** Each PR passes CI on its own (fmt, clippy, test, layering).

## Build directories
Each worktree has its own `target/` by default: safe, but several GB each once wgpu and polars arrive. To share one cache, set `CARGO_TARGET_DIR` to a common folder; builds then serialize on its lock, so only do this if disk space matters more than parallel builds.

## Clean up
```sh
git worktree remove ../mold.worktrees/graph     # after the branch is merged
git branch -d ws/graph
git worktree prune
```
