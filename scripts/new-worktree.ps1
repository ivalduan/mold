# Create a git worktree for one workstream, next to the main checkout.
#   .\scripts\new-worktree.ps1 graph          -> ..\mold.worktrees\graph on branch ws/graph
#   .\scripts\new-worktree.ps1 graph -Base ws/mol
# Workstream names are listed in docs/workstreams.md.
param(
    [Parameter(Mandatory = $true)][string]$Name,
    [string]$Base = "main"
)
$ErrorActionPreference = "Stop"

$known = @("spike","mol","chem","geom","graph","jobs","recipe","nodes","render",
           "ui-graph","ui-panels","app","workers-py","fixtures")
if ($known -notcontains $Name) {
    throw "Unknown workstream '$Name'. Known: $($known -join ', ') (see docs/workstreams.md)"
}

$root = git rev-parse --show-toplevel
$dir = Join-Path (Split-Path $root -Parent) "mold.worktrees\$Name"
$branch = "ws/$Name"

$exists = git -C $root branch --list $branch
if ($exists) {
    git -C $root worktree add $dir $branch
} else {
    git -C $root worktree add -b $branch $dir $Base
}
Write-Host "`nWorktree ready: $dir  (branch $branch)"
Write-Host "Brief: docs/workstreams.md#$Name, then the CLAUDE.md of the crate(s) you own."
