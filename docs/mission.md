# Mission and context

Distilled from the *Mold — Project Context Brief* ([source](https://claude.ai/artifact/QP7qEeJ57a43Gk39zfBvRg), 2026-11-23). **Decided** = treat as a constraint unless a human overrides it. **Proposed** = not yet confirmed. Market figures and competitor facts in the source are **unverified**; don't quote them externally.

## Mission (working draft)
Mold starts as a molecular workbench and grows into the hub for chemistry and bioinformatics: one visual, reproducible place where the field's engines meet, the way Houdini FX became the hub for visual effects. Built in the open.

## Thesis
The field has deep, mature libraries (RDKit, OpenBabel, MDAnalysis, BioPython) but no mature *tools* on top of them. Researchers glue libraries together with notebooks, CLI scripts and aging Java workflow apps. The opportunity: a Rust-powered, open-source, node-based hub that wraps those engines, shows every intermediate result live, and compiles every visual graph into versionable text.

Scientists distrust GUIs because point-and-click pipelines become unversionable black boxes that can't run on a cluster. Mold must solve that to win computational leads; hence graph ⇄ text.

## Scope over time
v0.1 is a workbench for 3D protein structures and ligands. The same hub (graph runtime, text recipes, plugin interface) then extends to small-molecule cheminformatics, sequence/multi-omics and molecular dynamics. **Each new domain arrives as a set of nodes, not a new product**: keep domain-specific code out of the core.

## What we are not
A from-scratch Rust rewrite of RDKit, or a company that sells "Rust" as the feature. Rust is the delivery advantage (speed, WebAssembly, local-first), not the pitch.

## Product principles
1. **Hub, not rewrite.** Rust builds the hub; chemistry and physics come from wrapped, battle-tested engines, plus pure-Rust kernels where mature.
2. **Graph ⇄ text, both ways.** Every graph serializes to a human-readable recipe that diffs in Git and runs headless. Text edits round-trip back.
3. **Every intermediate is visible.** Each node's output can be inspected live; non-destructive by default.
4. **Local-first, no IP leakage.** Runs on the user's machine, GPUs and clusters.
5. **Thin core, plugin edges.** External Python/CUDA models are worker nodes behind a stable interface.
6. **One codebase, native + web.**
7. **Open source, all of it.** Nothing a single scientist needs is paywalled.

## Proof-of-concept domain (decided)
3D structural biology and ML-driven protein–ligand modelling: the hybrid space where protein structures and small-molecule ligands meet (PROTACs, molecular glues, ADCs, cyclic peptides). Why:
- The biggest unsolved handoff in R&D: teams stitch together Python scripts, RDKit ligand prep, AI folding/docking and old desktop viewers with zero reproducibility.
- Avoids the RDKit-parity trap: in 3D, responsiveness and chaining AI/physics steps matter more than 2D drawing conventions.
- Open-source vacuum: the structure/AI tools users like are proprietary and cloud-only.
- Instant demo "aha": thousands of poses scrubbed smoothly.

**Fallback wedge** (if pivoting to small molecules): hit triage and virtual-library filtering for boutique computational CROs.

## The v0.1 demo: batch co-folding triage
Researchers generate many AI co-folded poses, then inspect them by hand to weed out physically broken ones. Mold does it as one graph on a public benchmark: ingest → co-fold/dock → physical sanity gate → interaction profile → linked 3D + SAR view. See [v0.1-plan.md](v0.1-plan.md).

## Business and governance (decided: everything is open source)
The company either grows into a service business on the OSS ecosystem, or into a non-profit foundation; final shape open. **Proposed:** keep engine and local authoring 100% open; monetize an enterprise layer (RBAC, audit trails, shared registries, burst execution to HPC/cloud).

## Go-to-market (decided goal)
After internal validation, run pilots with design partners to earn public case studies. Order: (1) boutique computational CROs, (2) seed/Series A AI protein startups, (3) academic structural biology cores and benchmark labs, (4) pharma, deferred until 3–5 public logos. Pilots run two tracks: private on the partner's target, public on a benchmark we publish.

## Glossary
**Co-folding**: predicting a protein structure together with a bound ligand (Boltz-2, Chai-1, AlphaFold 3). **Pose**: one predicted 3D placement of a ligand in a pocket. **PoseBusters**: benchmark and checks for physical validity of poses. **pLDDT / ipTM**: model confidence (per residue / interface). **SAR**: structure–activity relationship. **MSA**: multiple sequence alignment. **CRO**: contract research organization.
