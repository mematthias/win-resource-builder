#!/usr/bin/env python3
"""Compile every tests/fixtures/**/*.rc with rc.exe from the Windows SDK into a reference .res file.

With --llvm-rc, llvm-rc produces the references on any platform instead; they serve until rc.exe
regenerates them on Windows.

Each reference lands next to its .rc source and is committed; the golden tests in tests/golden
compare the crate output against them byte by byte on every platform. Rerun this script on
Windows whenever a .rc fixture or a canonical .ico fixture changes. Cursors have no .rc source:
their images are PNG, which rc.exe rejects in cursors (RC2176).
"""

from __future__ import annotations

import argparse
import platform
import re
import subprocess
import sys
from pathlib import Path

FIXTURES_DIR = Path(__file__).resolve().parent.parent / "tests" / "fixtures"
KITS_KEY = r"SOFTWARE\Microsoft\Windows Kits\Installed Roots"
HOST_DIRS = {"AMD64": "x64", "ARM64": "arm64", "x86": "x86"}
VERSION_DIR = re.compile(r"[0-9]+(?:\.[0-9]+){3}")


def pick_latest_rc(bin_dir: Path, host_dir: str) -> Path | None:
    """Return rc.exe of the highest SDK version below bin_dir that ships one for host_dir."""
    candidates = []
    for entry in bin_dir.iterdir():
        rc = entry / host_dir / "rc.exe"
        if VERSION_DIR.fullmatch(entry.name) and rc.is_file():
            # Numeric comparison per component, not as a string
            version = tuple(int(part) for part in entry.name.split("."))
            candidates.append((version, rc))
    return max(candidates)[1] if candidates else None


def find_rc() -> Path:
    import winreg  # Windows only; imported here so the module stays importable elsewhere

    # The SDK installer writes to the 32-bit view of the registry
    access = winreg.KEY_READ | winreg.KEY_WOW64_32KEY
    with winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, KITS_KEY, 0, access) as key:
        kits_root = Path(winreg.QueryValueEx(key, "KitsRoot10")[0])

    host_dir = HOST_DIRS.get(platform.machine())
    if host_dir is None:
        sys.exit(f"Unsupported host architecture '{platform.machine()}'")

    rc = pick_latest_rc(kits_root / "bin", host_dir)
    if rc is None:
        sys.exit(f"No rc.exe for '{host_dir}' found below {kits_root / 'bin'}")
    return rc


def compile_reference(rc: Path, flags: list[str], source: Path) -> tuple[Path, str | None]:
    """Compiles one fixture; returns the reference and the error output of rc.exe if it failed."""
    target = source.with_suffix(".res")
    # A stale reference must never survive a failed run
    target.unlink(missing_ok=True)

    # cwd is the case directory, so the .rc files name their inputs relative to it
    result = subprocess.run(
        [str(rc), *flags, "/fo", target.name, source.name],
        cwd=source.parent,
        capture_output=True,
        text=True,
        errors="replace",
        check=False,
    )
    if result.returncode != 0 or not target.is_file():
        target.unlink(missing_ok=True)
        return target, f"{result.stdout}{result.stderr}".strip()
    return target, None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--rc", type=Path, help="explicit rc.exe instead of the newest Windows SDK")
    parser.add_argument("--llvm-rc", type=Path, help="llvm-rc instead of rc.exe, on any platform")
    args = parser.parse_args()

    if args.llvm_rc is not None:
        # llvm-rc needs no preprocessor for the fixtures, which hold comments and plain statements only
        rc, flags = args.llvm_rc.resolve(strict=True), ["-no-preprocess"]
    elif sys.platform != "win32":
        sys.exit("rc.exe only runs on Windows; pass --llvm-rc for references on other platforms")
    else:
        rc, flags = (args.rc.resolve(strict=True) if args.rc else find_rc()), ["/nologo", "/x"]
    sources = sorted(FIXTURES_DIR.rglob("*.rc"))
    if not sources:
        sys.exit(f"No .rc files found below {FIXTURES_DIR}")

    # Compile every fixture, so a single run reports every failure
    failures = []
    for source in sources:
        target, error = compile_reference(rc, flags, source)
        name = source.relative_to(FIXTURES_DIR).as_posix()
        if error is None:
            print(f"{name} -> {target.name} ({target.stat().st_size} bytes)")
        else:
            print(f"{name} FAILED")
            failures.append((name, error))

    if failures:
        for name, error in failures:
            print(f"\nrc.exe failed for {name}:\n{error}", file=sys.stderr)
        return 1

    print(f"Generated with {rc}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
