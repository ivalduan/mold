#!/usr/bin/env -S uv run --script
# /// script
# requires-python = "==3.12.*"
# dependencies = ["gemmi==0.7.5"]
# ///
"""Generate the 300-pose set `mold-mol` loads in its Criterion bench.

    uv run fixtures/scripts/make_pose_set.py            # 300 poses
    uv run fixtures/scripts/make_pose_set.py --poses 500  # the 500-pose budget

A pose is one complete complex (receptor + ligand) in its own model, which is
how `mold-mol`'s arena lays poses out. 300 of them is roughly 60 MB of mmCIF,
far too much to commit, so the file is generated into fixtures/cache/ (not
committed) and its checksum is recorded in fixtures/checksums.sha256.

Each pose applies a rigid-body transform to the receptor (co-folding moves the
receptor too, which is why `pocket_rmsd` exists) and a larger rigid-body
transform to the ligand. Bond lengths and angles inside receptor and ligand are
therefore untouched: the poses are plausible as *input*, but they are not
predictions of anything. This file is for measuring load and display time, not
for scoring.

The perturbations come from `random.Random(SEED)`, so the file is byte-identical
on every platform and every run.
"""

from __future__ import annotations

import argparse
import math
import random

import gemmi

from _common import CACHE, FIXTURES, sha256_file, write_text

SEED = 20261124
RECEPTOR_ROTATION_DEG = 1.0
RECEPTOR_SHIFT_A = 0.5
LIGAND_ROTATION_DEG = 20.0
LIGAND_SHIFT_A = 1.5
LIGAND_COMP = "IRE"

REFERENCE = FIXTURES / "egfr" / "reference.cif"


def rotation(axis: tuple[float, float, float], angle_rad: float) -> list[list[float]]:
    norm = math.sqrt(sum(c * c for c in axis))
    x, y, z = (c / norm for c in axis)
    c, s = math.cos(angle_rad), math.sin(angle_rad)
    t = 1.0 - c
    return [
        [t * x * x + c, t * x * y - s * z, t * x * z + s * y],
        [t * x * y + s * z, t * y * y + c, t * y * z - s * x],
        [t * x * z - s * y, t * y * z + s * x, t * z * z + c],
    ]


def random_rigid(rng: random.Random, max_deg: float, max_shift: float):
    axis = (rng.gauss(0, 1), rng.gauss(0, 1), rng.gauss(0, 1))
    if axis == (0.0, 0.0, 0.0):
        axis = (1.0, 0.0, 0.0)
    matrix = rotation(axis, math.radians(rng.uniform(-max_deg, max_deg)))
    shift = tuple(rng.uniform(-max_shift, max_shift) for _ in range(3))
    return matrix, shift


def apply_rigid(atoms, matrix, shift, centre: gemmi.Position) -> None:
    for atom in atoms:
        p = atom.pos
        dx, dy, dz = p.x - centre.x, p.y - centre.y, p.z - centre.z
        atom.pos = gemmi.Position(
            centre.x + matrix[0][0] * dx + matrix[0][1] * dy + matrix[0][2] * dz + shift[0],
            centre.y + matrix[1][0] * dx + matrix[1][1] * dy + matrix[1][2] * dz + shift[1],
            centre.z + matrix[2][0] * dx + matrix[2][1] * dy + matrix[2][2] * dz + shift[2],
        )


def centroid(atoms) -> gemmi.Position:
    total = gemmi.Position(0, 0, 0)
    count = 0
    for atom in atoms:
        total += atom.pos
        count += 1
    return gemmi.Position(total.x / count, total.y / count, total.z / count)


def restart_atom_ids(cif: str) -> str:
    """Make `_atom_site.id` restart at 1 in every model.

    gemmi numbers atom sites continuously across models. pdbtbx compares the
    models of a multi-model file atom by atom and rejects continuous numbering
    with "Atoms in Models not corresponding", so `mold-mol` would refuse to
    read the pose set. The last column of the loop is pdbx_PDB_model_num.
    """
    lines = cif.split("\n")
    current_model = None
    serial = 0
    for index, line in enumerate(lines):
        if not (line.startswith("ATOM ") or line.startswith("HETATM ")):
            continue
        fields = line.split()
        if fields[-1] != current_model:
            current_model = fields[-1]
            serial = 0
        serial += 1
        fields[1] = str(serial)
        lines[index] = " ".join(fields)
    return "\n".join(lines)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--poses", type=int, default=300)
    args = parser.parse_args()

    if not REFERENCE.exists():
        raise SystemExit(f"{REFERENCE} is missing; run make_egfr_receptor.py first")
    base = gemmi.read_structure(str(REFERENCE))
    base.setup_entities()

    out = gemmi.Structure()
    out.name = f"egfr_{args.poses}poses"
    out.cell = base.cell
    out.spacegroup_hm = base.spacegroup_hm
    rng = random.Random(SEED)

    for index in range(args.poses):
        pose = base.clone()
        model = pose[0]
        model.num = index + 1
        receptor_atoms = [
            atom for chain in model for res in chain if res.name != LIGAND_COMP for atom in res
        ]
        ligand_atoms = [
            atom for chain in model for res in chain if res.name == LIGAND_COMP for atom in res
        ]
        if not ligand_atoms:
            raise SystemExit(f"no {LIGAND_COMP} residue in {REFERENCE}")
        matrix, shift = random_rigid(rng, RECEPTOR_ROTATION_DEG, RECEPTOR_SHIFT_A)
        apply_rigid(receptor_atoms, matrix, shift, centroid(receptor_atoms))
        matrix, shift = random_rigid(rng, LIGAND_ROTATION_DEG, LIGAND_SHIFT_A)
        apply_rigid(ligand_atoms, matrix, shift, centroid(ligand_atoms))
        out.add_model(model)

    out.setup_entities()
    path = CACHE / f"egfr-{args.poses}poses.cif"
    write_text(path, restart_atom_ids(out.make_mmcif_document().as_string()))
    atoms = sum(len(res) for model in out for chain in model for res in chain)
    print(
        f"{len(out)} poses, {atoms} atoms ({atoms // len(out)} per pose), "
        f"{path.stat().st_size / 1e6:.1f} MB"
    )
    print(f"  {sha256_file(path)}  cache/{path.name}  (not committed)")


if __name__ == "__main__":
    main()
