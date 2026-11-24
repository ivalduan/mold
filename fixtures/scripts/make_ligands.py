#!/usr/bin/env -S uv run --script
# /// script
# requires-python = "==3.12.*"
# dependencies = ["rdkit==2026.9.2"]
# ///
"""Build the EGFR 4-anilinoquinazoline ligand series.

    uv run fixtures/scripts/make_ligands.py

Series definition (see ADR-0007). A compound is in the set when all of these
hold:

  1. PubChem BioAssay has at least one IC50 for UniProt P00533 (human EGFR)
     with relation "=" and a positive value.
  2. It is a PubChem fast-substructure hit of 4-anilinoquinazoline
     (`c1ccc(Nc2ncnc3ccccc23)cc1`), the erlotinib/gefitinib chemotype.
  3. It is one fragment (no salts/mixtures), RDKit-parsable, and built only
     from H, C, N, O, F, S, Cl, Br.
  4. Its Bemis-Murcko scaffold is *exactly* the 4-anilinoquinazoline scaffold,
     i.e. the ring system is the shared core and only the substituents vary.
     This is what makes the set congeneric rather than "EGFR inhibitors".

Activity is the median of all qualifying IC50 values for that compound,
reported as pIC50 (= 9 - log10(IC50 in nM)).

Why PubChem and not the ChEMBL API: the ChEMBL REST service answered HTTP 500
for every request on 2026-11-24. PubChem redistributes the same depositor
rows (many of the EGFR assays here are ChEMBL deposits), so the licence note
in fixtures/egfr/README.md credits both.

Only public identifiers (a UniProt accession, a SMILES query, CIDs) are sent
out. Outputs:
  fixtures/egfr/ligands.smi               the series + the deliberately bad entries
  fixtures/egfr/ligands.bad.txt           which IDs must fail to parse, and why
  fixtures/egfr/ligands.provenance.toml   upstream snapshot, counts, versions
"""

from __future__ import annotations

import csv
import math
import statistics
import urllib.parse
import urllib.request
from collections import defaultdict
from datetime import date

import rdkit
from rdkit import Chem, RDLogger
from rdkit.Chem.Scaffolds import MurckoScaffold

from _common import CACHE, FIXTURES, USER_AGENT, download, log, sha256_file, write_text

RDLogger.DisableLog("rdApp.*")

OUT = FIXTURES / "egfr"
UNIPROT = "P00533"
CORE_SMILES = "c1ccc(Nc2ncnc3ccccc23)cc1"
ALLOWED_ELEMENTS = {1, 6, 7, 8, 9, 16, 17, 35}  # H C N O F S Cl Br
PROPERTY_CHUNK = 200

ACTIVITY_URL = (
    f"https://pubchem.ncbi.nlm.nih.gov/rest/pug/protein/accession/{UNIPROT}/concise/CSV"
)
SUBSTRUCTURE_URL = (
    "https://pubchem.ncbi.nlm.nih.gov/rest/pug/compound/fastsubstructure/smiles/cids/TXT"
    "?MatchIsotopes=false&MatchCharges=false&MaxRecords=500000"
)
PROPERTY_URL = (
    "https://pubchem.ncbi.nlm.nih.gov/rest/pug/compound/cid/property/"
    "SMILES,MolecularWeight,MolecularFormula/CSV"
)

# Deliberately unparsable SMILES, one per failure mode, so that chem's
# acceptance test can assert exactly which lines fail and keep going. The IDs
# are chosen so that a plain lexicographic sort interleaves them with the real
# ligands: a reader that aborts on the first failure fails the test early.
BAD_LIGANDS: list[tuple[str, str, str]] = [
    ("EGFR-0007-BAD", "COc1cc2c(Nc3cccc(Br)c3)ncnc2cc1OCCCN1CCOCC", "unclosed ring bond 1"),
    ("EGFR-0063-BAD", "COc1cc2c(Nc3ccc(F)c(Cl)c3)ncnc2cc1OC(", "unbalanced parenthesis"),
    ("EGFR-0121-BAD", "COc1cc2c(Nc3ccccc3)ncnc2cc1N(C)(C)(C)C", "nitrogen with five bonds"),
    ("EGFR-0200-BAD", "COc1cc2c(Nc3ccccc3)ncnc2cc1[Xx]", "unknown element symbol Xx"),
    ("EGFR-0288-BAD", "c1ccc2c(c1)ncnc2Nc1cccc1", "aromatic five-ring that cannot be kekulized"),
    ("EGFR-0377-BAD", "4-anilinoquinazoline", "a name, not SMILES"),
]


def fetch_activities() -> tuple[dict[int, list[float]], str, int]:
    """IC50 rows for the UniProt accession, grouped by CID."""
    path = download(ACTIVITY_URL, CACHE / f"pubchem_{UNIPROT}_concise.csv")
    per_cid: dict[int, list[float]] = defaultdict(list)
    total = 0
    with open(path, newline="", encoding="utf-8") as fh:
        for row in csv.DictReader(fh):
            total += 1
            if row["Activity Name"] != "IC50" or row["Activity Qualifier"] != "=":
                continue
            if not row["CID"]:
                continue
            try:
                micromolar = float(row["Activity Value [uM]"])
            except ValueError:
                continue
            if micromolar <= 0:
                continue
            per_cid[int(row["CID"])].append(micromolar)
    return per_cid, sha256_file(path), total


def fetch_substructure_cids() -> tuple[set[int], str]:
    path = CACHE / "pubchem_substructure_4_anilinoquinazoline.txt"
    if not path.exists():
        log(f"POST {SUBSTRUCTURE_URL}")
        body = urllib.parse.urlencode({"smiles": CORE_SMILES}).encode()
        req = urllib.request.Request(
            SUBSTRUCTURE_URL, data=body, headers={"User-Agent": USER_AGENT}
        )
        path.parent.mkdir(parents=True, exist_ok=True)
        with urllib.request.urlopen(req, timeout=300) as resp:
            path.write_bytes(resp.read())
    cids = {int(line) for line in path.read_text().split() if line.strip()}
    return cids, sha256_file(path)


def fetch_properties(cids: list[int]) -> dict[int, tuple[str, float, str]]:
    """SMILES, molecular weight and formula per CID, cached as one CSV."""
    path = CACHE / "pubchem_candidate_properties.csv"
    if not path.exists():
        rows: list[str] = []
        header = None
        for start in range(0, len(cids), PROPERTY_CHUNK):
            chunk = cids[start : start + PROPERTY_CHUNK]
            log(f"POST properties {start + 1}-{start + len(chunk)} of {len(cids)}")
            body = urllib.parse.urlencode({"cid": ",".join(map(str, chunk))}).encode()
            req = urllib.request.Request(
                PROPERTY_URL, data=body, headers={"User-Agent": USER_AGENT}
            )
            with urllib.request.urlopen(req, timeout=300) as resp:
                text = resp.read().decode()
            lines = text.splitlines()
            header = header or lines[0]
            rows.extend(lines[1:])
        path.parent.mkdir(parents=True, exist_ok=True)
        # Sorted by CID so the cached snapshot is order-independent.
        rows.sort(key=lambda line: int(line.split(",")[0]))
        path.write_text("\n".join([header or ""] + rows) + "\n", encoding="utf-8")
    out: dict[int, tuple[str, float, str]] = {}
    with open(path, newline="", encoding="utf-8") as fh:
        for row in csv.DictReader(fh):
            out[int(row["CID"])] = (row["SMILES"], float(row["MolecularWeight"]), row["MolecularFormula"])
    return out


def select(
    per_cid: dict[int, list[float]], props: dict[int, tuple[str, float, str]]
) -> tuple[list[dict], dict[str, int]]:
    core = Chem.MolFromSmiles(CORE_SMILES)
    core_scaffold = Chem.MolToSmiles(MurckoScaffold.GetScaffoldForMol(core))
    counts = defaultdict(int)
    kept = []
    for cid in sorted(props):
        smiles, mw, formula = props[cid]
        counts["candidates"] += 1
        mol = Chem.MolFromSmiles(smiles)
        if mol is None:
            counts["rdkit_rejected"] += 1
            continue
        if len(Chem.GetMolFrags(mol)) != 1:
            counts["multi_fragment"] += 1
            continue
        if not mol.HasSubstructMatch(core):
            counts["no_core_match"] += 1  # PubChem's match is looser than RDKit's
            continue
        if any(a.GetAtomicNum() not in ALLOWED_ELEMENTS for a in mol.GetAtoms()):
            counts["element_filter"] += 1
            continue
        if Chem.MolToSmiles(MurckoScaffold.GetScaffoldForMol(mol)) != core_scaffold:
            counts["scaffold_not_exact"] += 1
            continue
        values = per_cid[cid]
        median_um = statistics.median(values)
        kept.append(
            {
                "cid": cid,
                # RDKit's canonical SMILES, so the file does not depend on
                # PubChem's output conventions.
                "smiles": Chem.MolToSmiles(mol),
                "pic50": 9.0 - math.log10(median_um * 1000.0),
                "measurements": len(values),
                "mw": mw,
                "formula": formula,
                "heavy_atoms": mol.GetNumHeavyAtoms(),
            }
        )
        counts["selected"] += 1
    for index, entry in enumerate(kept, start=1):
        entry["id"] = f"EGFR-{index:04d}"
    return kept, dict(counts)


def check_bad() -> None:
    """A bad entry that quietly parses would silently weaken chem's test."""
    for ligand_id, smiles, reason in BAD_LIGANDS:
        if Chem.MolFromSmiles(smiles) is not None:
            raise SystemExit(f"{ligand_id} parses after all ({reason!r}): pick another")


def write_outputs(kept: list[dict], counts: dict[str, int], provenance: dict) -> None:
    lines = [f"{e['smiles']}\t{e['id']}\t{e['pic50']:.2f}\tpubchem:CID{e['cid']}" for e in kept]
    lines += [f"{smiles}\t{ligand_id}\tNA\tsynthetic:bad-smiles" for ligand_id, smiles, _ in BAD_LIGANDS]
    lines.sort(key=lambda line: line.split("\t")[1])
    write_text(OUT / "ligands.smi", "\n".join(lines) + "\n")

    bad = [
        "# Entries in egfr/ligands.smi that MUST fail to parse.",
        "#",
        "# Written by fixtures/scripts/make_ligands.py; do not edit by hand.",
        "# Verified against RDKit "
        f"{rdkit.__version__}: Chem.MolFromSmiles returns None for each of them.",
        "# Columns: id <TAB> reason <TAB> smiles",
        "",
    ]
    bad += [f"{ligand_id}\t{reason}\t{smiles}" for ligand_id, smiles, reason in BAD_LIGANDS]
    write_text(OUT / "ligands.bad.txt", "\n".join(bad) + "\n")

    pic50s = [e["pic50"] for e in kept]
    toml = [
        "# Provenance of egfr/ligands.smi (generated file).",
        "#",
        "# Written by fixtures/scripts/make_ligands.py; do not edit by hand.",
        "",
        f'generated = "{provenance["generated"]}"',
        f'rdkit_version = "{rdkit.__version__}"',
        f'core_smiles = "{CORE_SMILES}"',
        f'uniprot = "{UNIPROT}"',
        "",
        "[counts]",
        f"ligands = {len(kept)}",
        f"bad_smiles = {len(BAD_LIGANDS)}",
        f"lines_total = {len(kept) + len(BAD_LIGANDS)}",
        *[f"{key} = {value}" for key, value in sorted(counts.items())],
        f'activity_rows_scanned = {provenance["activity_rows"]}',
        f'substructure_hits = {provenance["substructure_hits"]}',
        "",
        "[activity]",
        'type = "pIC50 from the median of all qualifying PubChem IC50 values"',
        f"min = {min(pic50s):.2f}",
        f"max = {max(pic50s):.2f}",
        f"median = {statistics.median(pic50s):.2f}",
        f"with_multiple_measurements = {sum(1 for e in kept if e['measurements'] > 1)}",
        "",
        "[size]",
        f"heavy_atoms_min = {min(e['heavy_atoms'] for e in kept)}",
        f"heavy_atoms_max = {max(e['heavy_atoms'] for e in kept)}",
        f"molecular_weight_min = {min(e['mw'] for e in kept):.2f}",
        f"molecular_weight_max = {max(e['mw'] for e in kept):.2f}",
        "",
        "# Upstream snapshots, cached under fixtures/cache/ (not committed).",
        "# PubChem is updated continuously, so these checksums pin the snapshot",
        "# the committed ligands.smi was built from; the committed file is",
        "# authoritative, see fixtures/checksums.sha256.",
        "[[upstream]]",
        f'url = "{ACTIVITY_URL}"',
        f'sha256 = "{provenance["activity_sha256"]}"',
        "",
        "[[upstream]]",
        f'url = "{SUBSTRUCTURE_URL}"',
        f'body = "smiles={CORE_SMILES}"',
        f'sha256 = "{provenance["substructure_sha256"]}"',
        "",
    ]
    write_text(OUT / "ligands.provenance.toml", "\n".join(toml))


def main() -> None:
    check_bad()
    per_cid, activity_sha, activity_rows = fetch_activities()
    substructure, substructure_sha = fetch_substructure_cids()
    candidates = sorted(set(per_cid) & substructure)
    log(f"{len(per_cid)} CIDs with IC50, {len(substructure)} substructure hits, {len(candidates)} candidates")
    props = fetch_properties(candidates)
    kept, counts = select(per_cid, props)
    if not 200 <= len(kept) <= 500:
        raise SystemExit(
            f"selected {len(kept)} ligands, outside the 200-500 the benchmark asks for; "
            "revisit the filters in ADR-0007 before freezing"
        )
    write_outputs(
        kept,
        counts,
        {
            "generated": date.today().isoformat(),
            "activity_sha256": activity_sha,
            "activity_rows": activity_rows,
            "substructure_sha256": substructure_sha,
            "substructure_hits": len(substructure),
        },
    )
    print(f"{len(kept)} ligands + {len(BAD_LIGANDS)} unparsable entries")
    for name in ("ligands.smi", "ligands.bad.txt", "ligands.provenance.toml"):
        print(f"  {sha256_file(OUT / name)}  egfr/{name}")


if __name__ == "__main__":
    main()
