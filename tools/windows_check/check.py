#!/usr/bin/env python3
"""Build an executable and a DLL with win-resource-builder resources on Windows and check that Windows reads
every icon and cursor in them.

The build script of tools/windows_check generates icons whose PNG images oxipng reduces to every
form it knows (RGBA, RGB, grayscale, palette, low bit depths, tRNS), plus SVG and fixture icons and
cursors, and links them into windows_check.exe and win_resource_builder_payload.dll. The executable then loads
every image with LookupIconIdFromDirectoryEx, CreateIconFromResourceEx, LoadImageW and
PrivateExtractIconsW from itself, from the DLL and from the written .ico/.cur files, and compares
the pixels Windows returns with the stored PNG.

Run it from any directory:  python tools/windows_check/check.py [--target TRIPLE ...]
"""

from __future__ import annotations

import argparse
import platform
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent


def build(target: str | None, profile: str) -> Path:
    """Builds both binaries; returns the directory that holds them."""
    command = ["cargo", "build", "--profile", profile]
    if target:
        command += ["--target", target]
    subprocess.run(command, cwd=ROOT, check=True)
    directory = ROOT / "target"
    if target:
        directory /= target
    return directory / ("debug" if profile == "dev" else profile)


def check(target: str | None, profile: str, report_dir: Path) -> int:
    label = target or "host"
    print(f"== {label} ==", flush=True)
    directory = build(target, profile)
    executable = directory / "windows_check.exe"
    try:
        result = subprocess.run([str(executable)], cwd=directory, capture_output=True, text=True, errors="replace")
    except OSError as err:
        # CreateProcess fails before the first instruction if Windows rejects the manifest
        print(f"{executable} does not start: {err}", file=sys.stderr)
        return 1

    header = f"Windows {platform.version()} ({platform.machine()}), target {label}, profile {profile}\n"
    report = header + result.stdout + result.stderr
    print(report)
    path = report_dir / f"report_{label}.txt"
    path.write_text(report, encoding="utf-8")
    print(f"Report: {path}")
    return result.returncode


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--target",
        action="append",
        help="Rust target triple, e.g. x86_64-pc-windows-gnu to check GNU ld as well; repeatable, default host",
    )
    parser.add_argument("--profile", default="release", help="Cargo profile, default release")
    parser.add_argument("--report-dir", type=Path, default=ROOT, help="directory of the report files")
    args = parser.parse_args()

    if sys.platform != "win32":
        sys.exit("The check needs Windows: it tests the image loader of user32")
    failures = [target for target in (args.target or [None]) if check(target, args.profile, args.report_dir) != 0]
    if failures:
        print(f"Failed: {', '.join(target or 'host' for target in failures)}", file=sys.stderr)
        return 1
    print("Every image loads with the pixels it stores")
    return 0


if __name__ == "__main__":
    sys.exit(main())
