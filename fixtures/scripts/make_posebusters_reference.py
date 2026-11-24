#!/usr/bin/env -S uv run --script
# /// script
# requires-python = "==3.12.*"
# dependencies = ["posebusters==0.6.5", "rdkit==2026.9.2", "gemmi==0.7.5", "pandas==2.3.3"]
# ///
"""Produce the PoseBusters reference CSV that `mold-geom` cross-validates against.

    uv run fixtures/scripts/make_posebusters_reference.py

CI has no Python, so this runs offline on a developer box and commits its
output. Three public EGFR complexes (PDB, CC0) give the crystal poses; each one
is then deliberately broken five ways, so the reference contains real failures
and not only passes.

Outputs:
  fixtures/posebusters/inputs/<pdb>_protein.pdb   receptor, solvent/buffer/ligands removed
  fixtures/posebusters/inputs/<pdb>_<pose>.sdf    the ligand poses, with explicit bonds
  fixtures/posebusters/inputs/manifest.toml       what each pose is, and its checksum
  fixtures/posebusters/reference.csv              PoseBusters full report, one row per pose
"""

from __future__ import annotations

import math
import sys
from pathlib import Path

import gemmi
import pandas as pd
import posebusters
import rdkit
from posebusters import PoseBusters
from rdkit import Chem
from rdkit.Geometry import Point3D

from _common import CACHE, FIXTURES, clear_anisou, download, log, sha256_file, write_text

OUT = FIXTURES / "posebusters"
INPUTS = OUT / "inputs"

SOLVENT = {"HOH", "DOD"}
# Crystallisation additives and ions in these three entries.
BUFFER = {"MES", "NA", "CL", "SO4", "GOL", "EDO", "ACT", "PO4", "MG", "ZN", "CA", "DMS"}

# (PDB id, chain, ligand comp id, what it is)
COMPLEXES = [
    ("1M17", "A", "AQ4", "erlotinib, non-covalent"),
    ("4WKQ", "A", "IRE", "gefitinib, non-covalent"),
    ("4G5J", "A", "0WN", "afatinib, covalently bound to Cys797"),
]

CLASH_SHIFT_A = 4.0
FAR_SHIFT_A = 30.0
STRETCHED_BOND_A = 3.0
INTERNAL_CLASH_A = 0.6
RING_PUCKER_A = 0.7

POSES = {
    "crystal": "the deposited ligand coordinates, unmodified",
    "clash": f"whole ligand translated {CLASH_SHIFT_A} A towards the protein centroid",
    "far": f"whole ligand translated {FAR_SHIFT_A} A away from the protein centroid",
    "stretched": f"one acyclic single bond stretched to {STRETCHED_BOND_A} A",
    "internal-clash": (
        f"one terminal heavy atom moved to {INTERNAL_CLASH_A} A from a non-bonded atom"
    ),
    "puckered": f"one aromatic ring atom pushed {RING_PUCKER_A} A out of the ring plane",
}


def entry_path(pdb_id: str) -> Path:
    """The deposited entry: reuse fixtures/structures/ when it is already there."""
    committed = FIXTURES / "structures" / f"{pdb_id}.cif"
    if committed.exists():
        return committed
    return download(
        f"https://files.rcsb.org/download/{pdb_id}.cif", CACHE / f"{pdb_id}.cif"
    )


def is_polymer_residue(res: gemmi.Residue) -> bool:
    info = gemmi.find_tabulated_residue(res.name)
    return bool(info and (info.is_amino_acid() or info.is_nucleic_acid()))


def write_protein(pdb_id: str) -> tuple[Path, tuple[float, float, float]]:
    """Protein-only PDB (all polymer chains), plus the protein centroid."""
    st = gemmi.read_structure(str(entry_path(pdb_id)))
    st.setup_entities()
    st.remove_alternative_conformations()
    st.remove_hydrogens()
    out = gemmi.Structure()
    out.name = f"{pdb_id}_protein"
    out.cell = st.cell
    out.spacegroup_hm = st.spacegroup_hm
    model = gemmi.Model("1")
    total = gemmi.Position(0, 0, 0)
    count = 0
    for chain in st[0]:
        kept = gemmi.Chain(chain.name)
        for res in chain:
            if res.name in SOLVENT or res.name in BUFFER or not is_polymer_residue(res):
                continue
            kept.add_residue(res)
            for atom in res:
                total += atom.pos
                count += 1
        if len(kept):
            model.add_chain(kept)
    out.add_model(model)
    out.setup_entities()
    clear_anisou(out)
    path = INPUTS / f"{pdb_id}_protein.pdb"
    write_text(path, out.make_pdb_string())
    centroid = (total.x / count, total.y / count, total.z / count)
    return path, centroid


def crystal_ligand(pdb_id: str, chain_id: str, comp_id: str) -> Chem.Mol:
    """The ligand instance with explicit bonds, from the RCSB model server.

    The server stamps its own version into the SDF header, so the response is
    cached but not checksummed; what we commit is our own RDKit output.
    """
    url = (
        f"https://models.rcsb.org/v1/{pdb_id.lower()}/ligand"
        f"?auth_asym_id={chain_id}&label_comp_id={comp_id}&encoding=sdf"
    )
    raw = download(url, CACHE / f"{pdb_id}_{comp_id}.model-server.sdf")
    mol = Chem.MolFromMolFile(str(raw), removeHs=False, sanitize=True)
    if mol is None:
        raise SystemExit(f"RDKit could not read {comp_id} of {pdb_id}")
    mol.SetProp("_Name", f"{pdb_id}_{comp_id}")
    return mol


def positions(mol: Chem.Mol) -> list[tuple[float, float, float]]:
    conf = mol.GetConformer()
    return [tuple(conf.GetAtomPosition(i)) for i in range(mol.GetNumAtoms())]


def with_positions(mol: Chem.Mol, pos: list[tuple[float, float, float]]) -> Chem.Mol:
    out = Chem.Mol(mol)
    conf = out.GetConformer()
    for i, (x, y, z) in enumerate(pos):
        conf.SetAtomPosition(i, Point3D(float(x), float(y), float(z)))
    return out


def unit_towards(mol: Chem.Mol, centroid: tuple[float, float, float]) -> tuple[float, float, float]:
    pos = positions(mol)
    lx = sum(p[0] for p in pos) / len(pos)
    ly = sum(p[1] for p in pos) / len(pos)
    lz = sum(p[2] for p in pos) / len(pos)
    vec = (centroid[0] - lx, centroid[1] - ly, centroid[2] - lz)
    norm = math.sqrt(sum(c * c for c in vec))
    return (vec[0] / norm, vec[1] / norm, vec[2] / norm)


def translated(mol: Chem.Mol, direction: tuple[float, float, float], distance: float) -> Chem.Mol:
    return with_positions(
        mol,
        [
            (p[0] + direction[0] * distance, p[1] + direction[1] * distance, p[2] + direction[2] * distance)
            for p in positions(mol)
        ],
    )


def stretched(mol: Chem.Mol) -> Chem.Mol:
    """Stretch the first acyclic single bond between heavy atoms."""
    for bond in mol.GetBonds():
        if bond.IsInRing() or bond.GetBondType() != Chem.BondType.SINGLE:
            continue
        i, j = bond.GetBeginAtomIdx(), bond.GetEndAtomIdx()
        if mol.GetAtomWithIdx(i).GetAtomicNum() == 1 or mol.GetAtomWithIdx(j).GetAtomicNum() == 1:
            continue
        pos = positions(mol)
        pi, pj = pos[i], pos[j]
        vec = (pj[0] - pi[0], pj[1] - pi[1], pj[2] - pi[2])
        length = math.sqrt(sum(c * c for c in vec))
        scale = STRETCHED_BOND_A / length
        pos[j] = (pi[0] + vec[0] * scale, pi[1] + vec[1] * scale, pi[2] + vec[2] * scale)
        return with_positions(mol, pos)
    raise SystemExit("no acyclic single bond to stretch")


def internal_clash(mol: Chem.Mol) -> Chem.Mol:
    """Move a terminal heavy atom next to an atom it is not bonded to."""
    dist = Chem.GetDistanceMatrix(mol)
    pos = positions(mol)
    for atom in mol.GetAtoms():
        if atom.GetDegree() != 1 or atom.GetAtomicNum() == 1:
            continue
        i = atom.GetIdx()
        for j in range(mol.GetNumAtoms()):
            if dist[i][j] < 4 or mol.GetAtomWithIdx(j).GetAtomicNum() == 1:
                continue
            target = pos[j]
            pos[i] = (target[0] + INTERNAL_CLASH_A, target[1], target[2])
            return with_positions(mol, pos)
    raise SystemExit("no terminal atom to clash")


def puckered(mol: Chem.Mol) -> Chem.Mol:
    """Push one atom of an aromatic ring out of its plane."""
    for ring in mol.GetRingInfo().AtomRings():
        if not all(mol.GetAtomWithIdx(i).GetIsAromatic() for i in ring):
            continue
        pos = positions(mol)
        a, b, c = (pos[ring[0]], pos[ring[1]], pos[ring[2]])
        u = (b[0] - a[0], b[1] - a[1], b[2] - a[2])
        v = (c[0] - a[0], c[1] - a[1], c[2] - a[2])
        normal = (
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        )
        norm = math.sqrt(sum(x * x for x in normal))
        normal = tuple(x / norm for x in normal)
        i = ring[3]
        pos[i] = tuple(pos[i][k] + normal[k] * RING_PUCKER_A for k in range(3))
        return with_positions(mol, pos)
    raise SystemExit("no aromatic ring to pucker")


def ascii_column(name: str) -> str:
    """PoseBusters names some columns with `<=`, `-` and Angstrom signs.

    A Rust test reading this CSV should not have to spell those, so the header
    is normalised to lowercase ASCII snake_case.
    """
    out = name.replace("≤", "le").replace("å", "a").replace("-", "_")
    out = out.encode("ascii", "ignore").decode()
    while "__" in out:
        out = out.replace("__", "_")
    return out.strip("_").lower()


def write_sdf(mol: Chem.Mol, path: Path) -> None:
    write_text(path, Chem.MolToMolBlock(mol, kekulize=True) + "$$$$\n")


def main() -> None:
    sys.stdout.reconfigure(encoding="utf-8")  # the summary prints check names
    INPUTS.mkdir(parents=True, exist_ok=True)
    buster = PoseBusters(config="redock")
    frames = []
    manifest = [
        "# PoseBusters reference inputs (generated file).",
        "#",
        "# Written by fixtures/scripts/make_posebusters_reference.py; do not edit by hand.",
        f'posebusters_version = "{posebusters.__version__}"',
        f'rdkit_version = "{rdkit.__version__}"',
        f'gemmi_version = "{gemmi.__version__}"',
        "",
    ]

    for pdb_id, chain_id, comp_id, description in COMPLEXES:
        protein_path, centroid = write_protein(pdb_id)
        truth = crystal_ligand(pdb_id, chain_id, comp_id)
        direction = unit_towards(truth, centroid)
        variants = {
            "crystal": Chem.Mol(truth),
            "clash": translated(truth, direction, CLASH_SHIFT_A),
            "far": translated(truth, direction, -FAR_SHIFT_A),
            "stretched": stretched(truth),
            "internal-clash": internal_clash(truth),
            "puckered": puckered(truth),
        }
        truth_path = INPUTS / f"{pdb_id}_{comp_id}_crystal.sdf"
        manifest += [
            "[[complex]]",
            f'pdb_id = "{pdb_id}"',
            f'chain = "{chain_id}"',
            f'ligand_comp_id = "{comp_id}"',
            f'ligand = "{description}"',
            f'protein = "inputs/{protein_path.name}"',
            f'protein_sha256 = "{sha256_file(protein_path)}"',
            "",
        ]
        for pose, mol in variants.items():
            path = INPUTS / f"{pdb_id}_{comp_id}_{pose}.sdf"
            write_sdf(mol, path)
            log(f"busting {pdb_id} {pose}")
            df = buster.bust(
                mol_pred=[path], mol_true=truth_path, mol_cond=protein_path, full_report=True
            )
            df = df.reset_index(drop=True)
            df.insert(0, "pdb_id", pdb_id)
            df.insert(1, "ligand_comp_id", comp_id)
            df.insert(2, "pose", pose)
            df.insert(3, "ligand_file", f"inputs/{path.name}")
            df.insert(4, "protein_file", f"inputs/{protein_path.name}")
            df.insert(5, "true_ligand_file", f"inputs/{truth_path.name}")
            df.insert(6, "perturbation", POSES[pose])
            frames.append(df)
            manifest += [
                "[[pose]]",
                f'pdb_id = "{pdb_id}"',
                f'pose = "{pose}"',
                f'file = "inputs/{path.name}"',
                f'perturbation = "{POSES[pose]}"',
                f'sha256 = "{sha256_file(path)}"',
                "",
            ]

    table = pd.concat(frames, ignore_index=True)
    # Drop PoseBusters' own file/name columns: our own columns carry that, and
    # theirs would embed absolute developer paths.
    table = table.drop(columns=[c for c in ("file", "molecule") if c in table.columns])
    # The committed receptors are protein-only, so PoseBusters has no cofactor
    # or water reference entities: those checks are either empty or vacuously
    # False (and in 4WKQ the modified residue CSX 797 is picked up as an
    # "organic cofactor", which is an artefact of writing it as HETATM). Drop
    # the whole family rather than ship values geom must not trust.
    dropped = [
        c
        for c in table.columns
        if c.endswith(("_organic_cofactors", "_inorganic_cofactors", "_waters"))
        or table[c].isna().all()
    ]
    table = table.drop(columns=dropped)
    table = table.rename(columns=ascii_column)
    table = table.sort_values(["pdb_id", "pose"]).reset_index(drop=True)
    csv = table.to_csv(index=False, lineterminator="\n", float_format="%.6g")
    write_text(OUT / "reference.csv", csv)
    write_text(INPUTS / "manifest.toml", "\n".join(manifest))

    bool_cols = [c for c in table.columns if table[c].dtype == bool]
    print(f"{len(table)} poses, {len(table.columns)} columns, {len(bool_cols)} boolean checks")
    for _, row in table.iterrows():
        failed = [c for c in bool_cols if not row[c]]
        print(f"  {row['pdb_id']:5s} {row['pose']:15s} failures: {', '.join(failed) or 'none'}")
    print(f"  {sha256_file(OUT / 'reference.csv')}  posebusters/reference.csv")


if __name__ == "__main__":
    main()
