# fixtures/structures

Parser golden files for `crates/mol` (WS-01). Each entry is stored in both PDB and mmCIF
format, unmodified, exactly as downloaded from RCSB. Golden values in
`crates/mol/tests/` were derived from the PDB text independently of the Rust parser.

| ID | Files | Source | Licence | Why it is here |
|---|---|---|---|---|
| 1M17 | `1M17.pdb` (247 KB), `1M17.cif` (300 KB) | https://files.rcsb.org/download/1M17.pdb, https://files.rcsb.org/download/1M17.cif | CC0 1.0 (wwPDB data policy) | EGFR kinase domain with erlotinib (ligand `AQ4`, 29 heavy atoms), non-covalent. The likely benchmark target (ADR-0007). Also has 2 residues with altlocs. |
| 4G5J | `4G5J.pdb` (467 KB), `4G5J.cif` (541 KB) | https://files.rcsb.org/download/4G5J.pdb, https://files.rcsb.org/download/4G5J.cif | CC0 1.0 (wwPDB data policy) | EGFR kinase domain with afatinib (`0WN`, 34 atoms) **covalently bound** to Cys797 SG (LINK record, C30–SG 1.84 Å). Two extra non-covalent afatinib copies (`0WM`). |
| 1EJG | `1EJG.pdb` (122 KB), `1EJG.cif` (148 KB) | https://files.rcsb.org/download/1EJG.pdb, https://files.rcsb.org/download/1EJG.cif | CC0 1.0 (wwPDB data policy) | Crambin at 0.54 Å: 17 of 46 residues have alternate locations (A/B/C), including microheterogeneity (PRO/SER 22, LEU/ILE 25) and residues where B, not A, has the higher occupancy (ASN 12, PHE 13, ASP 43). Exercises the altloc policy. |

Downloaded 2026-11-24. PDB archive data is released under CC0 1.0
(https://www.wwpdb.org/about/usage-policies). Total size about 1.8 MB.
