# 0001. Record architecture decisions

- Status: Accepted
- Date: 2026-11-24

## Context
Mold is an open project developed in parallel worktrees, much of it by coding agents. Choices about forks (canvas, viewport, GPUI lineage, solvers) must be visible to every stream and to future contributors.

## Decision
Every fork choice gets a numbered ADR in `adr/`. Week 0's only deliverable is ADRs, not application code.

## Consequences
Agents check `adr/` before re-opening a settled question. A change of mind supersedes an ADR rather than editing it.
