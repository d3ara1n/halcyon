#!/usr/bin/env python3
"""Generate heterogeneous-domain DTS variants from a platform DTS.

Strips f/d extensions from the selected cpu nodes' riscv,isa-extensions.
The kernel trusts the DT as the capability fact (conservatively correct:
hardware FP simply goes unused), producing Base64-only domains alongside
D64 domains — exercising domain partitioning, weakest-compatible default
placement and D64 eligibility routing on real QEMU hardware.

Usage:
    make-hetero-dts.py INPUT OUTPUT cpu [cpu ...]   # cpu = hart id, or "all"
"""

from pathlib import Path
import sys


def transform(lines: list[str], targets: set[int] | None) -> list[str]:
    out: list[str] = []
    cpu = None  # current cpu@N block's hart id; None outside cpu blocks
    depth = 0  # brace depth inside the current cpu block (nested subnodes)
    for line in lines:
        stripped = line.strip()
        if cpu is None and stripped.startswith("cpu@") and stripped.endswith("{"):
            cpu = int(stripped[4:-1], 0)
            depth = 1
        elif cpu is not None:
            if stripped.endswith("{"):
                depth += 1
            if stripped.startswith("}"):  # `}` 或 `};` 都是块结束
                depth -= 1
                if depth == 0:
                    cpu = None
        if (
            cpu is not None
            and (targets is None or cpu in targets)
            and "riscv,isa-extensions" in line
        ):
            line = line.replace('"f", "d", ', "").replace('"d", "f", ', "")
            line = line.replace('"f", ', "").replace('"d", ', "")
        out.append(line)
    return out


def main() -> int:
    if len(sys.argv) < 4:
        raise SystemExit(__doc__)
    source, destination = Path(sys.argv[1]), Path(sys.argv[2])
    selectors = sys.argv[3:]
    targets = None if "all" in selectors else {int(s, 0) for s in selectors}
    lines = transform(source.read_text().splitlines(), targets)
    destination.write_text("\n".join(lines) + "\n")
    print(f"hetero dts written: {destination} (f/d stripped from "
          f"{'all cpus' if targets is None else sorted(targets)})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
