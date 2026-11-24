#!/usr/bin/env -S uv run --script
# /// script
# requires-python = "==3.12.*"
# dependencies = ["gemmi==0.7.5", "rdkit==2026.9.2"]
# ///
"""Build the EGFR benchmark receptor, reference crystal and pocket definition.

    uv run fixtures/scripts/make_egfr_receptor.py

Source: PDB 4WKQ, EGFR kinase domain (wild type, UniProt P00533 residues
696-1022) with gefitinib, 1.85 A, one chain, no mutations. PDB data is CC0 1.0.
See ADR-0007 for why this entry.

Outputs (all deterministic; re-running yields byte-identical files):
  fixtures/egfr/receptor.cif          chain A polymer only, no ligand/solvent
  fixtures/egfr/reference.cif         chain A polymer + gefitinib (ligand IRE)
  fixtures/egfr/reference_ligand.sdf  gefitinib crystal pose with explicit bonds
  fixtures/egfr/pocket.toml           pocket residue list for pocket RMSD
"""

from __future__ import annotations

import gemmi
from rdkit import Chem

from _common import CACHE, FIXTURES, clear_anisou, download, sha256_file, write_text

PDB_ID = "4WKQ"
LIGAND_COMP = "IRE"  # gefitinib
CHAIN = "A"
POCKET_CUTOFF_A = 5.0

ENTRY_URL = f"https://files.rcsb.org/download/{PDB_ID}.cif"
ENTRY_SHA256 = "b23c36532327cc3a812e02a6ea0a6731e465538df24c1132b1e102a23dbdb73e"
# The ModelServer response carries its own version banner, so its checksum is
# not stable across server upgrades; we re-emit the molecule with RDKit and
# record the checksum of our own output instead.
LIGAND_URL = (
    f"https://models.rcsb.org/v1/{PDB_ID.lower()}/ligand"
    f"?auth_asym_id={CHAIN}&label_comp_id={LIGAND_COMP}&encoding=sdf"
)

OUT = FIXTURES / "egfr"
SOLVENT = {"HOH", "DOD"}
BUFFER = {"MES", "NA"}  # crystallisation additives, not part of the target


def is_polymer_residue(res: gemmi.Residue) -> bool:
    info = gemmi.find_tabulated_residue(res.name)
    return bool(info and (info.is_amino_acid() or info.is_nucleic_acid()))


def load_entry() -> gemmi.Structure:
    path = download(ENTRY_URL, CACHE / f"{PDB_ID}.cif", ENTRY_SHA256)
    st = gemmi.read_structure(str(path))
    st.setup_entities()
    clear_anisou(st)
    if len(st) != 1:
        raise SystemExit(f"{PDB_ID}: expected 1 model, found {len(st)}")
    return st


def subset(st: gemmi.Structure, name: str, keep_ligand: bool) -> gemmi.Structure:
    """Copy `st` keeping chain A's polymer (and optionally the ligand)."""
    out = gemmi.Structure()
    out.name = name
    out.cell = st.cell
    out.spacegroup_hm = st.spacegroup_hm
    out.resolution = st.resolution
    model = gemmi.Model("1")
    chain = gemmi.Chain(CHAIN)
    kept_ligand = 0
    for res in st[0][CHAIN]:
        if res.name in SOLVENT or res.name in BUFFER:
            continue
        if is_polymer_residue(res):
            chain.add_residue(res)
        elif res.name == LIGAND_COMP and keep_ligand:
            chain.add_residue(res)
            kept_ligand += 1
    model.add_chain(chain)
    out.add_model(model)
    out.setup_entities()
    if keep_ligand and kept_ligand != 1:
        raise SystemExit(f"{PDB_ID}: expected 1 {LIGAND_COMP} copy, found {kept_ligand}")
    return out


def write_cif(st: gemmi.Structure, path) -> None:
    # gemmi's own writer opens the file in text mode, which would emit CRLF on
    # Windows; serialise to a string and write it ourselves with LF endings.
    write_text(path, st.make_mmcif_document().as_string())


def ligand_sdf() -> Chem.Mol:
    raw = download(LIGAND_URL, CACHE / f"{PDB_ID}_{LIGAND_COMP}.model-server.sdf")
    mol = Chem.MolFromMolFile(str(raw), removeHs=False, sanitize=True)
    if mol is None:
        raise SystemExit(f"RDKit could not read the {LIGAND_COMP} SDF from ModelServer")
    mol.SetProp("_Name", f"{PDB_ID}_{LIGAND_COMP}")
    mol.SetProp("pdb_id", PDB_ID)
    mol.SetProp("comp_id", LIGAND_COMP)
    mol.SetProp("smiles", Chem.MolToSmiles(mol))
    return mol


def pocket_residues(st: gemmi.Structure) -> list[tuple[gemmi.Residue, float]]:
    lig = [
        res for res in st[0][CHAIN] if res.name == LIGAND_COMP
    ]
    if len(lig) != 1:
        raise SystemExit(f"{PDB_ID}: expected 1 {LIGAND_COMP} copy in the deposited model")
    lig_pos = [a.pos for a in lig[0] if a.element != gemmi.Element("H")]
    found: list[tuple[gemmi.Residue, float]] = []
    for res in st[0][CHAIN]:
        if not is_polymer_residue(res):
            continue
        best = min(
            (p.dist(q) for a in res if a.element != gemmi.Element("H") for p in (a.pos,) for q in lig_pos),
            default=None,
        )
        if best is not None and best <= POCKET_CUTOFF_A:
            found.append((res, best))
    return found


def pocket_toml(pocket: list[tuple[gemmi.Residue, float]]) -> str:
    seq_ids = [res.seqid.num for res, _ in pocket]
    lines = [
        "# Pocket definition for the EGFR benchmark (generated file).",
        "#",
        "# Written by fixtures/scripts/make_egfr_receptor.py; do not edit by hand.",
        "# Pocket RMSD is computed over these residues after superposing the",
        "# predicted complex on reference.cif; see ADR-0007.",
        "",
        f'pdb_id = "{PDB_ID}"',
        f'chain = "{CHAIN}"',
        f'ligand_comp_id = "{LIGAND_COMP}"',
        'ligand_name = "gefitinib"',
        'numbering = "author (auth_seq_id), identical to UniProt P00533 numbering"',
        f"cutoff_angstrom = {POCKET_CUTOFF_A}",
        'cutoff_definition = "any heavy atom of the residue within cutoff of any ligand heavy atom"',
        f"residue_count = {len(pocket)}",
        "# Flat list, for consumers that only need the residue numbers.",
        "residues = [" + ", ".join(str(n) for n in seq_ids) + "]",
        "",
        "[rmsd]",
        'atoms = "CA"  # recommended default: C-alpha of the residues above',
        'alternative_atoms = "all-heavy"  # for side-chain sensitive comparisons',
        "tolerance_angstrom = 0.1  # agreement expected between implementations",
        "",
        "[cross_reference]",
        "# A second crystal of the same kinase domain, already committed under",
        "# fixtures/structures/. Use it when a non-degenerate pocket RMSD is",
        "# wanted (receptor.cif comes from 4WKQ, so 4WKQ-vs-4WKQ is 0 by",
        "# construction). 1M17 is EGFR + erlotinib, the chemotype of the",
        "# ligand series, at 2.60 A; its kinase domain uses the same author",
        "# numbering, chain A.",
        'path = "../structures/1M17.cif"',
        'pdb_id = "1M17"',
        'ligand_comp_id = "AQ4"',
        "",
    ]
    for res, dist in pocket:
        lines += [
            "[[residue]]",
            f"seq_id = {res.seqid.num}",
            f'name = "{res.name}"',
            f"min_ligand_distance_angstrom = {dist:.2f}",
            "",
        ]
    return "\n".join(lines)


def main() -> None:
    st = load_entry()

    receptor = subset(st, f"{PDB_ID}_receptor", keep_ligand=False)
    write_cif(receptor, OUT / "receptor.cif")

    reference = subset(st, f"{PDB_ID}_reference", keep_ligand=True)
    write_cif(reference, OUT / "reference.cif")

    mol = ligand_sdf()
    write_text(OUT / "reference_ligand.sdf", Chem.MolToMolBlock(mol) + _props(mol))

    pocket = pocket_residues(st)
    write_text(OUT / "pocket.toml", pocket_toml(pocket))

    n_atoms = sum(len(r) for ch in receptor[0] for r in ch)
    print(
        f"{PDB_ID}: receptor {len(receptor[0][CHAIN])} residues / {n_atoms} atoms, "
        f"pocket {len(pocket)} residues within {POCKET_CUTOFF_A} A, "
        f"ligand {mol.GetNumAtoms()} atoms / {mol.GetNumBonds()} bonds"
    )
    for name in ("receptor.cif", "reference.cif", "reference_ligand.sdf", "pocket.toml"):
        print(f"  {sha256_file(OUT / name)}  egfr/{name}")


def _props(mol: Chem.Mol) -> str:
    """SDF property block, written by hand so the field order is stable."""
    out = []
    for key in ("pdb_id", "comp_id", "smiles"):
        out.append(f">  <{key}>\n{mol.GetProp(key)}\n")
    return "\n".join(out) + "\n$$$$\n"


if __name__ == "__main__":
    main()
