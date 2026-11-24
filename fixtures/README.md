# fixtures/

Frozen public benchmark data. Owned by workstream WS-13; see [docs/workstreams.md#fixtures](../docs/workstreams.md#fixtures) and ADR-0007.

Planned layout:
```
fixtures/
  structures/        parser golden files (altlocs, covalent ligand, …)
  egfr/              benchmark target (default choice, pending ADR-0007)
    receptor.cif
    reference.cif    crystal for pocket RMSD
    ligands.smi      200–500 ligands, a few deliberately unparsable
    target.a3m       cached MSA, so tests make no network call
  mock-cofold/       real poses + timings replayed by workers/mock-cofold
  posebusters/       reference CSV produced offline by Python PoseBusters
```
Only public data with a licence that permits redistribution. Record source and licence per dataset in ADR-0007.
