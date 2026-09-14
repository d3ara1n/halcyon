#!/usr/bin/env python3
"""Run the shared Rust ELF admission against built user images."""

from __future__ import annotations

from pathlib import Path
import subprocess
import sys


def host_target() -> str:
    output = subprocess.check_output(["rustc", "-vV"], text=True)
    for line in output.splitlines():
        if line.startswith("host: "):
            return line.removeprefix("host: ")
    raise RuntimeError("rustc did not report its host target")


def main() -> int:
    if len(sys.argv) < 2:
        print("usage: audit-user-elf.py ELF...", file=sys.stderr)
        return 2
    root = Path(__file__).resolve().parents[1]
    command = [
        "cargo",
        "run",
        "--quiet",
        "--manifest-path",
        str(root / "shared/elf/Cargo.toml"),
        "--features",
        "host-audit",
        "--target",
        host_target(),
        "--bin",
        "audit_user_elf",
        "--",
        *sys.argv[1:],
    ]
    try:
        return subprocess.run(command, cwd=root, check=False).returncode
    except (OSError, RuntimeError) as error:
        print(f"user ELF audit failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
