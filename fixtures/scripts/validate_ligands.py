#!/usr/bin/env -S uv run --script
# /// script
# requires-python = "==3.12.*"
# dependencies = ["rdkit==2026.9.2"]
# ///
"""Check the committed ligand series against RDKit.

    uv run fixtures/scripts/validate_ligands.py

This is the same assertion `mold-chem`'s acceptance test makes, run here with
the reference implementation: every ligand in egfr/ligands.smi parses except
the entries listed in egfr/ligands.bad.txt, which must all fail. Exits non-zero
if that is not exactly true, so the fixture cannot rot unnoticed.
"""

from __future__ import annotations

import sys

import rdkit
from rdkit import Chem, RDLogger

from _common import FIXTURES

RDLogger.DisableLog("rdApp.*")

LIGANDS = FIXTURES / "egfr" / "ligands.smi"
BAD = FIXTURES / "egfr" / "ligands.bad.txt"


def main() -> int:
    expected_bad = {}
    for line in BAD.read_text(encoding="utf-8").splitlines():
        if not line.strip() or line.startswith("#"):
            continue
        ligand_id, reason, _smiles = line.split("\t")
        expected_bad[ligand_id] = reason

    parsed, failed = 0, {}
    ids: list[str] = []
    for number, line in enumerate(LIGANDS.read_text(encoding="utf-8").splitlines(), start=1):
        smiles, ligand_id, activity, source = line.split("\t")
        ids.append(ligand_id)
        if Chem.MolFromSmiles(smiles) is None:
            failed[ligand_id] = f"line {number}"
        else:
            parsed += 1
        if ligand_id in expected_bad:
            if activity != "NA":
                print(f"{ligand_id}: an unparsable entry should carry activity NA")
                return 1
        elif not source.startswith("pubchem:CID"):
            print(f"{ligand_id}: unexpected source {source!r}")
            return 1

    problems = []
    if sorted(failed) != sorted(expected_bad):
        problems.append(
            f"failing ids {sorted(failed)} != manifest {sorted(expected_bad)}"
        )
    if ids != sorted(ids):
        problems.append("ligands.smi is not sorted by id")
    if len(set(ids)) != len(ids):
        problems.append("duplicate ids in ligands.smi")

    print(f"RDKit {rdkit.__version__}")
    print(f"  {len(ids)} lines, {parsed} parsed, {len(failed)} failed as intended")
    for ligand_id in sorted(failed):
        print(f"  {ligand_id}  {failed[ligand_id]}  ({expected_bad.get(ligand_id, 'NOT IN MANIFEST')})")
    for problem in problems:
        print(f"PROBLEM: {problem}", file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    raise SystemExit(main())
