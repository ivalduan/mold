#!/usr/bin/env -S uv run --script
# /// script
# requires-python = "==3.12.*"
# dependencies = ["plip==3.0.1", "openbabel-wheel==3.1.1.23"]
# ///
"""Produce the PLIP reference CSV that `mold-geom`'s contact profiler is checked against.

    uv run fixtures/scripts/make_plip_reference.py

PLIP (Salentin et al., NAR 2015; Adasme et al., NAR 2021) is GPL-2.0. It runs
here offline, once, to produce data; nothing in mold links or ships it
(ADR-0008). CI has no Python: the Rust test only reads the CSV.

Inputs: the PoseBusters fixture set (fixtures/posebusters/inputs), so both
references describe exactly the same atoms. Only the poses that are rigid
copies of the deposited ligand are profiled (`crystal`, `clash`, `far`): PLIP
reads the ligand from PDB records and re-perceives its bond orders from the
geometry, which is only trustworthy for undistorted ligands.

Outputs:
  fixtures/plip/reference.csv           one row per interaction PLIP reports
  fixtures/cache/plip/<pdb>_<pose>.pdb  the complexes PLIP read (not committed)
"""

from __future__ import annotations

import csv
import io
import sys
from pathlib import Path

import openbabel
from plip.basic import config
from plip.structure.preparation import PDBComplex

from _common import CACHE, FIXTURES, log, write_text

INPUTS = FIXTURES / "posebusters" / "inputs"
OUT = FIXTURES / "plip"
WORK = CACHE / "plip"

COMPLEXES = [("1M17", "AQ4"), ("4WKQ", "IRE"), ("4G5J", "0WN")]
POSES = ["crystal", "clash", "far"]
LIGAND_CHAIN = "Z"
LIGAND_RESNUM = 900

COLUMNS = [
    "complex",
    "pose",
    "interaction",
    "ligand_role",
    "ligand_atoms",
    "receptor_residue",
    "receptor_atoms",
    "distance",
    "angle",
    "offset",
]


def read_sdf(path: Path) -> tuple[list[tuple[str, float, float, float]], list[tuple[int, int]]]:
    """First record of a V2000 SDF: (element, x, y, z) per atom, 0-based bonds."""
    lines = path.read_text().splitlines()
    n_atoms, n_bonds = int(lines[3][0:3]), int(lines[3][3:6])
    atoms = []
    for line in lines[4 : 4 + n_atoms]:
        atoms.append((line[31:34].strip(), float(line[0:10]), float(line[10:20]), float(line[20:30])))
    bonds = []
    for line in lines[4 + n_atoms : 4 + n_atoms + n_bonds]:
        bonds.append((int(line[0:3]) - 1, int(line[3:6]) - 1))
    return atoms, bonds


Key = tuple[int, int, int]


def key(x: float, y: float, z: float) -> Key:
    """Atoms are matched back by coordinates (PDB precision, 1e-3 A): PLIP's own
    atom indices go through its PDB fixing and altloc handling, so they are not
    the file's serials."""
    return (round(x * 1000), round(y * 1000), round(z * 1000))


def compose(pdb: str, lig: str, pose: str) -> tuple[str, dict[Key, int], dict[Key, tuple[str, str]]]:
    """The receptor plus the ligand pose as one PDB.

    Returns the text, ligand coordinates -> 0-based SDF index, and receptor
    coordinates -> ("chain:resname:resnum", atom name).
    """
    out = io.StringIO()
    serial = 0
    receptor: dict[Key, tuple[str, str]] = {}
    for line in (INPUTS / f"{pdb}_protein.pdb").read_text().splitlines():
        if line.startswith(("ATOM", "HETATM")):
            serial += 1
            out.write(f"{line[:6]}{serial:5d}{line[11:]}\n")
            chain, resname, resnum = line[21], line[17:20].strip(), line[22:27].strip()
            k = key(float(line[30:38]), float(line[38:46]), float(line[46:54]))
            receptor[k] = (f"{chain}:{resname}:{resnum}", line[12:16].strip())
    out.write("TER\n")
    atoms, bonds = read_sdf(INPUTS / f"{pdb}_{lig}_{pose}.sdf")
    ligand: dict[Key, int] = {}
    first = serial + 1
    counts: dict[str, int] = {}
    for i, (el, x, y, z) in enumerate(atoms):
        serial += 1
        counts[el] = counts.get(el, 0) + 1
        name = f"{el.upper()}{counts[el]}"
        line = (
            f"HETATM{serial:5d} {name:<4} {lig:>3} {LIGAND_CHAIN}{LIGAND_RESNUM:4d}    "
            f"{x:8.3f}{y:8.3f}{z:8.3f}  1.00  0.00          {el.upper():>2}\n"
        )
        out.write(line)
        ligand[key(float(line[30:38]), float(line[38:46]), float(line[46:54]))] = i
    neighbours: dict[int, list[int]] = {i: [] for i in range(len(atoms))}
    for a, b in bonds:
        neighbours[a].append(b)
        neighbours[b].append(a)
    for i, nb in neighbours.items():
        for k in range(0, len(nb), 4):
            rest = "".join(f"{first + j:5d}" for j in nb[k : k + 4])
            out.write(f"CONECT{first + i:5d}{rest}\n")
    out.write("END\n")
    return out.getvalue(), ligand, receptor


def fmt(x: float | None) -> str:
    return "" if x is None else f"{x:.4f}"


def profile(pdb: str, lig: str, pose: str) -> list[dict[str, str]]:
    text, ligand, receptor = compose(pdb, lig, pose)
    path = WORK / f"{pdb}_{pose}.pdb"
    write_text(path, text)
    mol = PDBComplex()
    mol.output_path = str(WORK)
    mol.load_pdb(str(path))
    for ligand_obj in mol.ligands:
        if ligand_obj.hetid == lig and ligand_obj.chain == LIGAND_CHAIN:
            mol.characterize_complex(ligand_obj)
    site = f"{lig}:{LIGAND_CHAIN}:{LIGAND_RESNUM}"
    if site not in mol.interaction_sets:
        raise SystemExit(f"{pdb} {pose}: PLIP found no ligand {site}; got {list(mol.interaction_sets)}")
    s = mol.interaction_sets[site]

    def lig_atoms(atoms) -> str:
        return " ".join(str(i) for i in sorted(ligand[key(*a.coords)] for a in atoms))

    def rec(atoms) -> tuple[str, str]:
        found = [receptor[key(*a.coords)] for a in atoms]
        residues = {r for r, _ in found}
        assert len(residues) == 1, residues
        return residues.pop(), " ".join(sorted(n for _, n in found))

    rows = []

    def row(interaction, role, lig_idx, rec_idx, distance, angle=None, offset=None):
        residue, names = rec(rec_idx)
        rows.append(
            {
                "complex": pdb,
                "pose": pose,
                "interaction": interaction,
                "ligand_role": role,
                "ligand_atoms": lig_atoms(lig_idx),
                "receptor_residue": residue,
                "receptor_atoms": names,
                "distance": fmt(distance),
                "angle": fmt(angle),
                "offset": fmt(offset),
            }
        )

    for hb in s.hbonds_ldon:
        row("hbond", "donor", [hb.d], [hb.a], hb.distance_ad, hb.angle)
    for hb in s.hbonds_pdon:
        row("hbond", "acceptor", [hb.a], [hb.d], hb.distance_ad, hb.angle)
    for sb in s.saltbridge_lneg:
        row("saltbridge", "negative", sb.negative.atoms, sb.positive.atoms, sb.distance)
    for sb in s.saltbridge_pneg:
        row("saltbridge", "positive", sb.positive.atoms, sb.negative.atoms, sb.distance)
    for ps in s.pistacking:
        row("pistack", ps.type, ps.ligandring.atoms, ps.proteinring.atoms, ps.distance, ps.angle, ps.offset)
    for pc in s.pication_laro:  # ligand ring, protein cation
        row("pication", "ring", pc.ring.atoms, pc.charge.atoms, pc.distance, None, pc.offset)
    for pc in s.pication_paro:  # protein ring, ligand cation
        row("pication", "cation", pc.charge.atoms, pc.ring.atoms, pc.distance, None, pc.offset)
    rows.sort(key=lambda r: (r["interaction"], r["ligand_role"], r["ligand_atoms"], r["receptor_residue"], r["receptor_atoms"]))
    log(f"{pdb} {pose}: {len(rows)} interactions")
    return rows


def without_inchikey() -> None:
    """openbabel-wheel ships without the InChI plugin; PLIP only uses the ligand's
    InChIKey as a label, so answer that one format with an empty string."""
    from openbabel import pybel

    write = pybel.Molecule.write

    def patched(self, format="smi", *args, **kwargs):
        if format == "inchikey":
            return ""
        return write(self, format, *args, **kwargs)

    pybel.Molecule.write = patched


def main() -> None:
    without_inchikey()
    config.NOFIXFILE = True
    config.QUIET = True
    rows = []
    for pdb, lig in COMPLEXES:
        for pose in POSES:
            rows.extend(profile(pdb, lig, pose))
    buf = io.StringIO()
    w = csv.DictWriter(buf, fieldnames=COLUMNS, lineterminator="\n")
    w.writeheader()
    w.writerows(rows)
    write_text(OUT / "reference.csv", buf.getvalue())
    log(f"PLIP 3.0.1, Open Babel {openbabel.__version__ if hasattr(openbabel, '__version__') else '3.1.1'}, Python {sys.version.split()[0]}")


if __name__ == "__main__":
    main()
