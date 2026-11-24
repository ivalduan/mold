#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.10"
# dependencies = []
# ///
"""Verify the committed fixtures, and fetch the ones too large to commit.

    uv run fixtures/fetch.py                 # verify committed files (no network)
    uv run fixtures/fetch.py fetch           # download the upstream snapshots
    uv run fixtures/fetch.py update-checksums

Standard library only, so this runs on any Python 3.10+ and needs no
environment. Everything it downloads lands in fixtures/cache/, which is not
committed; the CI subset is committed and checksummed in
fixtures/checksums.sha256.

Only public identifiers (PDB ids, a UniProt accession, a SMILES query) are ever
sent out. Nothing here uploads anything.
"""

from __future__ import annotations

import argparse
import hashlib
import sys
import urllib.request
from pathlib import Path

FIXTURES = Path(__file__).resolve().parent
CACHE = FIXTURES / "cache"
CHECKSUMS = FIXTURES / "checksums.sha256"
USER_AGENT = "mold-fixtures/0.1 (open-source structural-biology workbench)"

# Upstream snapshots the generation scripts read. The committed fixtures are
# derived from exactly these; see ADR-0007.
#   (url, cache file, sha256 or None when upstream is not byte-stable)
UPSTREAM: list[tuple[str, str, str | None]] = [
    (
        "https://files.rcsb.org/download/4WKQ.cif",
        "4WKQ.cif",
        "b23c36532327cc3a812e02a6ea0a6731e465538df24c1132b1e102a23dbdb73e",
    ),
    (
        "https://models.rcsb.org/v1/4wkq/ligand?auth_asym_id=A&label_comp_id=IRE&encoding=sdf",
        "4WKQ_IRE.model-server.sdf",
        None,  # the response carries the model server's version banner
    ),
    (
        "https://models.rcsb.org/v1/1m17/ligand?auth_asym_id=A&label_comp_id=AQ4&encoding=sdf",
        "1M17_AQ4.model-server.sdf",
        None,
    ),
    (
        "https://models.rcsb.org/v1/4g5j/ligand?auth_asym_id=A&label_comp_id=0WN&encoding=sdf",
        "4G5J_0WN.model-server.sdf",
        None,
    ),
    (
        "https://pubchem.ncbi.nlm.nih.gov/rest/pug/protein/accession/P00533/concise/CSV",
        "pubchem_P00533_concise.csv",
        None,  # PubChem is updated continuously
    ),
]

# Artifacts that are generated rather than downloaded, too large to commit.
#   (cache file, sha256, how to make it)
GENERATED: list[tuple[str, str, str]] = [
    (
        "egfr-300poses.cif",
        "e385a21bd305666d8039f68b6acc46a922441b9d220142e8e9ce8aa6882c8952",
        "uv run fixtures/scripts/make_pose_set.py",
    ),
]


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def committed_files() -> list[Path]:
    skip = {"cache", "__pycache__", ".venv"}
    out = []
    for path in sorted(FIXTURES.rglob("*")):
        if not path.is_file():
            continue
        if any(part in skip for part in path.relative_to(FIXTURES).parts):
            continue
        if path.name in {"checksums.sha256"} or path.suffix == ".pyc":
            continue
        out.append(path)
    return out


def verify() -> int:
    if not CHECKSUMS.exists():
        print(f"{CHECKSUMS} is missing; run `update-checksums`", file=sys.stderr)
        return 1
    expected = {}
    for line in CHECKSUMS.read_text(encoding="utf-8").splitlines():
        if not line.strip() or line.startswith("#"):
            continue
        digest, name = line.split(maxsplit=1)
        name = name.strip().lstrip("*")
        if name.startswith("cache/"):
            continue  # generated, handled below
        expected[name] = digest
    bad = 0
    seen = set()
    for path in committed_files():
        name = path.relative_to(FIXTURES).as_posix()
        seen.add(name)
        if name not in expected:
            print(f"UNLISTED  {name}")
            bad += 1
        elif sha256_file(path) != expected[name]:
            print(f"CHANGED   {name}")
            bad += 1
    for name in sorted(set(expected) - seen):
        print(f"MISSING   {name}")
        bad += 1
    cached = 0
    for name, digest, command in GENERATED:
        path = CACHE / name
        if not path.exists():
            print(f"not generated yet: cache/{name}  ->  {command}")
        elif sha256_file(path) != digest:
            print(f"CHANGED   cache/{name} (regenerate with: {command})")
            bad += 1
        else:
            cached += 1
    print(
        f"{len(seen)} committed files checked, {bad} problem(s)"
        f"{f', {cached} generated artifact(s) verified' if cached else ''}"
    )
    return 1 if bad else 0


def fetch() -> int:
    CACHE.mkdir(parents=True, exist_ok=True)
    bad = 0
    for url, name, digest in UPSTREAM:
        path = CACHE / name
        if not path.exists():
            print(f"GET {url}")
            request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
            with urllib.request.urlopen(request, timeout=300) as response:
                path.write_bytes(response.read())
        got = sha256_file(path)
        if digest and got != digest:
            print(f"CHECKSUM MISMATCH  cache/{name}\n  expected {digest}\n  got      {got}")
            bad += 1
        else:
            print(f"ok  cache/{name}  {got[:16]}…")
    print("\nto regenerate the artifacts that are too large to commit:")
    for name, _, command in GENERATED:
        print(f"  {command}    -> fixtures/cache/{name}")
    return 1 if bad else 0


def update_checksums() -> int:
    lines = [
        "# sha256 of every committed file under fixtures/ (sha256sum format).",
        "# Regenerate with: uv run fixtures/fetch.py update-checksums",
        "# Verify with:     uv run fixtures/fetch.py",
        "",
    ]
    for path in committed_files():
        lines.append(f"{sha256_file(path)}  {path.relative_to(FIXTURES).as_posix()}")
    lines.append("")
    lines.append("# Generated, not committed (see fixtures/README.md).")
    for name, digest, command in GENERATED:
        lines.append(f"{digest}  cache/{name}")
    with open(CHECKSUMS, "w", encoding="utf-8", newline="\n") as fh:
        fh.write("\n".join(lines) + "\n")
    print(f"wrote {CHECKSUMS.relative_to(FIXTURES.parent)} ({len(committed_files())} files)")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "action",
        nargs="?",
        default="verify",
        choices=["verify", "fetch", "update-checksums"],
    )
    action = parser.parse_args().action
    return {"verify": verify, "fetch": fetch, "update-checksums": update_checksums}[action]()


if __name__ == "__main__":
    raise SystemExit(main())
