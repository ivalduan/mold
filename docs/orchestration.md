# Orchestrating parallel workstreams

How we run two or more workstreams at once: an orchestrator session briefs independent agents, one per worktree, then reviews and lands their work as separate PRs. This is the process used for PR #1 (`graph`) and PR #2 (`mol`).

## Roles
| Who | Does | Doesn't |
|---|---|---|
| Product owner (human) | Chooses the items, settles design questions, merges PRs | |
| Orchestrator (main Claude session, in the main checkout) | Proposes items, creates worktrees, writes briefs, steers running agents, reviews, rebases, pushes, opens PRs, records decisions (ADRs) | Merge PRs; settle open design questions alone |
| Workstream agent (one per worktree) | Implements its brief in its own worktree, commits locally with sign-off, reports back | Push, open PRs, touch other crates, the main checkout, or contract types |

## 1. Pick the items
- Use [workstreams.md](workstreams.md): items whose "Can start" condition holds and whose open decisions don't block them. `jobs` builds on [ADR-0009](../adr/0009-execution-flavours.md) (accepted); its contract change is on `main` (#19).
- Prefer pairs that share no files. The root `Cargo.toml` / `THIRD_PARTY.md` / `Cargo.lock` are the only expected overlap.
- Ask the product owner whenever the choice or scope isn't obvious: which items, the PR scope, and whether a design question blocks.

## 2. Create worktrees
`.\scripts\new-worktree.ps1 <name>`: creates `..\mold.worktrees\<name>` on branch `ws/<name>`.

## 3. Brief each agent
A background general-purpose agent per worktree. The brief must stand alone (the agent starts cold). Template:

```
You are the engineer for workstream <WS-ID> `<name>` of Mold (<one line on Mold>).
I am the orchestrator. You report back to me; I review and push.
Do NOT push, do NOT open PRs, do NOT touch the main checkout.

## Where you work
- Worktree: C:\Users\ivana\Projects\mold.worktrees\<name> (branch ws/<name>). Only this directory.
- Windows. cargo: export PATH="$HOME/.cargo/bin:$PATH" (Bash) or the PowerShell equivalent.

## Read first
CLAUDE.md, crates/<crate>/CLAUDE.md, the relevant docs/architecture.md sections and ADRs, docs/workstreams.md, docs/worktrees.md.

## Goal: one reviewable first PR = public API + acceptance tests + first working implementation
<numbered scope; what's explicitly out of scope>

## Tests first
<the acceptance tests from the crate's CLAUDE.md, made concrete>

## Rules
- Allowed dependencies; never GPUI / async runtime in core crates.
- New crate: root Cargo.toml [workspace.dependencies] + THIRD_PARTY.md row, in its own commit, touching only those two files.
- Contract types (root CLAUDE.md list): don't change them, report the need instead.
- Update the crate's CLAUDE.md checkboxes plus a "Design notes" section.
- Before each commit: cargo fmt --all --check; cargo clippy --workspace --all-targets -- -D warnings; cargo test --workspace; bash scripts/check-layering.sh.
- git commit -s; logical commits; last line "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>".
- Don't stall on ambiguity: pick what fits the docs, note it, continue.

## Report back
1. commits + files 2. public API 3. acceptance tests pass/deferred 4. design decisions 5. contract changes / cross-workstream needs 6. check output 7. PR title + description
```

## 4. Steer while they run
If a design decision changes mid-run, message the agent right away. Say what to **pause** and what still holds either way, so it never sits idle, then send the decision as a new scope. Record the decision in an ADR on `main`.

## 5. Review (in the agent's worktree)
- [ ] Re-run all four checks yourself; don't trust the report.
- [ ] Re-run timing-sensitive tests ~10× to catch flakiness: `for i in $(seq 10); do cargo test -q -p <crate> --test <t>; done`.
- [ ] `git diff main..HEAD -- Cargo.toml THIRD_PARTY.md`: only the declared dependencies, licences recorded.
- [ ] Transitive dependency surprises: `cargo tree -p <crate> -e normal --prefix none | grep -E '^(tokio|gpui)'`.
- [ ] Verify key claims independently (e.g. golden-test facts against the raw fixture files, not through the parser).
- [ ] Contract types untouched; deviations from the brief or an ADR are justified.
- [ ] Bring docs in line (ADR implementation notes, root `CLAUDE.md` contract list) in a final commit on the branch.

## 6. Land
1. `git fetch origin && git rebase origin/main` in the worktree.
2. Conflicts in the root `Cargo.toml`: keep both sides' entries. `Cargo.lock`: `git checkout --ours Cargo.lock`, then `cargo metadata` to regenerate it. Never hand-merge the lock file.
3. Re-run the checks with `--locked`.
4. `git push --force-with-lease origin ws/<name>` (plain `push -u` the first time).
5. Write the PR body to a file, then `gh pr create --base main --head ws/<name> --title ... --body-file ...`. End the body with the Claude Code attribution line.
6. Watch CI: `gh pr checks <n> --watch`. The product owner merges. When two PRs touch the dependency hotspot, merge one and rebase the other.

## 7. After merge
Update the status in [workstreams.md](workstreams.md). Remove the worktree once its branch is merged: `git worktree remove ../mold.worktrees/<name>; git branch -d ws/<name>`.

## Tooling on the dev box
- cargo: `%USERPROFILE%\.cargo\bin`, not on the Git Bash PATH.
- gh: `C:\Program Files\GitHub CLI`, not on the Git Bash PATH; authenticated as the repo owner.
- Remote: `git@github.com:ivalduan/mold.git` (provisional, may move to a Mold org).
