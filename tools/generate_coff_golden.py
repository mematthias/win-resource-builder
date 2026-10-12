#!/usr/bin/env python3
"""Generate the COFF references tests/fixtures/<kind>/<case>.x64.obj with llvm-cvtres.

Every golden case writes the .res form of this crate when WIN_RESOURCE_BUILDER_GOLDEN_DUMP is set, and
llvm-cvtres converts each of them with a fixed time stamp. The golden tests then require the COFF
form of this crate to match byte for byte. Runs on every platform with LLVM installed; rerun after
a change of a golden case or of the image encoders and commit the references.
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FIXTURES_DIR = ROOT / "tests" / "fixtures"
# Unversioned name first, then the versioned names of Debian and Ubuntu packages, newest first
CVTRES_NAMES = ["llvm-cvtres"] + [f"llvm-cvtres-{version}" for version in range(30, 13, -1)]


def find_cvtres(explicit: Path | None) -> Path:
    if explicit is not None:
        return explicit.resolve(strict=True)
    for name in CVTRES_NAMES:
        found = shutil.which(name)
        if found is not None:
            return Path(found)
    sys.exit("llvm-cvtres not found; put LLVM on PATH or pass --cvtres")


def dump_cases(dump_dir: Path) -> list[Path]:
    """Runs the golden tests, which write the .res form of every case into dump_dir."""
    env = dict(os.environ, WIN_RESOURCE_BUILDER_GOLDEN_DUMP=str(dump_dir))
    # The rc.exe comparisons fail while their references are missing; every case dumps before comparing
    result = subprocess.run(
        ["cargo", "test", "--quiet", "--test", "golden"],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        errors="replace",
        check=False,
    )
    dumps = sorted(dump_dir.rglob("*.res"))
    if not dumps:
        sys.exit(f"No case wrote its .res form; the golden tests did not run:\n{result.stdout}{result.stderr}")
    return dumps


def convert(cvtres: Path, source: Path, target: Path) -> None:
    # A stale reference must never survive a failed run
    target.unlink(missing_ok=True)
    result = subprocess.run(
        [str(cvtres), "/MACHINE:X64", "/TIMESTAMP:0", f"/OUT:{target}", str(source)],
        capture_output=True,
        text=True,
        errors="replace",
        check=False,
    )
    if result.returncode != 0 or not target.is_file():
        sys.exit(f"llvm-cvtres failed for {source.name}:\n{result.stdout}{result.stderr}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--cvtres", type=Path, help="explicit llvm-cvtres instead of the first one on PATH")
    args = parser.parse_args()

    cvtres = find_cvtres(args.cvtres)
    with tempfile.TemporaryDirectory() as tmp:
        for dump in dump_cases(Path(tmp)):
            kind = dump.parent.name
            target = FIXTURES_DIR / kind / f"{dump.stem}.x64.obj"
            convert(cvtres, dump, target)
            print(f"{kind}/{target.name} ({target.stat().st_size} bytes)")

    print(f"Generated with {cvtres}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
