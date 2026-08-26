#!/usr/bin/env python3
"""Build the canonical eRhino BootPackage v1 image."""

from __future__ import annotations

import argparse
import os
from pathlib import Path
import struct
import sys

MAGIC = b"ERHBOOT\0"
VERSION = 1
HEADER_LEN = 64
PAGE_SIZE = 4096
HEADER = struct.Struct("<8sHHI6Q")


def align_up(value: int, alignment: int) -> int:
    return (value + alignment - 1) & ~(alignment - 1)


def build(initial_elf: bytes, payload: bytes) -> bytes:
    if not initial_elf:
        raise ValueError("initial ELF must not be empty")
    init_off = HEADER_LEN
    payload_off = align_up(init_off + len(initial_elf), PAGE_SIZE)
    total_len = align_up(payload_off + len(payload), PAGE_SIZE)
    header = HEADER.pack(
        MAGIC,
        VERSION,
        HEADER_LEN,
        0,
        total_len,
        init_off,
        len(initial_elf),
        payload_off,
        len(payload),
        0,
    )
    image = bytearray(total_len)
    image[:HEADER_LEN] = header
    image[init_off : init_off + len(initial_elf)] = initial_elf
    image[payload_off : payload_off + len(payload)] = payload
    return bytes(image)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--init", required=True, type=Path, help="initial process ELF")
    parser.add_argument("--payload", required=True, type=Path, help="opaque init payload")
    parser.add_argument("--output", required=True, type=Path, help="output BootPackage")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        initial_elf = args.init.read_bytes()
        payload = args.payload.read_bytes()
        image = build(initial_elf, payload)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        temporary = args.output.with_name(f".{args.output.name}.tmp")
        temporary.write_bytes(image)
        os.replace(temporary, args.output)
    except (OSError, ValueError) as error:
        print(f"BootPackage build failed: {error}", file=sys.stderr)
        return 1
    print(
        f"BootPackage built: {args.output} ({len(image)} bytes, "
        f"init {len(initial_elf)} bytes, payload {len(payload)} bytes)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
