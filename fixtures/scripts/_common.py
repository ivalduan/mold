"""Shared helpers for the fixture generation scripts.

Imported by the sibling scripts (`uv run` puts the script's directory on
`sys.path`). Only the standard library is used here, so this module adds no
dependency to any script that imports it.
"""

from __future__ import annotations

import hashlib
import sys
import urllib.request
from pathlib import Path

FIXTURES = Path(__file__).resolve().parent.parent
CACHE = FIXTURES / "cache"

# Only public identifiers are ever sent out; see fixtures/README.md.
USER_AGENT = "mold-fixtures/0.1 (open-source structural-biology workbench)"


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def download(url: str, dest: Path, expect_sha256: str | None = None) -> Path:
    """Download `url` to `dest` once, then verify its checksum.

    `dest` lives under fixtures/cache/ and is never committed.
    """
    dest.parent.mkdir(parents=True, exist_ok=True)
    if not dest.exists():
        log(f"GET {url}")
        req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
        with urllib.request.urlopen(req, timeout=120) as resp:
            data = resp.read()
        dest.write_bytes(data)
    got = sha256_file(dest)
    if expect_sha256 is not None and got != expect_sha256:
        raise SystemExit(
            f"checksum mismatch for {dest.name}\n  expected {expect_sha256}\n  got      {got}\n"
            "The upstream file changed (PDB entries can be re-versioned). "
            "Check the new file, then update the checksum and ADR-0007."
        )
    return dest


def clear_anisou(st) -> None:
    """Drop anisotropic displacement parameters.

    They roughly double the size of a high-resolution entry and nothing in
    mold reads them; isotropic B-factors are kept.
    """
    import gemmi  # only the scripts that call this depend on gemmi

    zero = gemmi.SMat33f(0, 0, 0, 0, 0, 0)
    for model in st:
        for chain in model:
            for res in chain:
                for atom in res:
                    atom.aniso = zero


def write_text(path: Path, text: str) -> None:
    """Write text with LF endings, so checksums match on every platform."""
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        fh.write(text)
    log(f"wrote {path.relative_to(FIXTURES.parent)}  ({path.stat().st_size} B)  {sha256_file(path)[:16]}…")


def log(msg: str) -> None:
    print(msg, file=sys.stderr)
