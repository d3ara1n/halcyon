#!/usr/bin/env python3
"""按领域选择内核契约验收；压力和平台变体由调用者显式选择。"""
import argparse
from dataclasses import dataclass
import os
import shlex
import subprocess
import sys
import time


@dataclass(frozen=True)
class Route:
    sources: str
    suite: str = ""
    mode: int = 0
    build: str = "release"
    fp: bool = False
    model: str = "virt"
    platform: str = ""


ROUTES = {}
for group in ("memory", "ipc", "process", "tunnel", "time"):
    ROUTES[group, "basic"] = Route(f"acceptance suite-{group}", group)
for group in ("mmio", "irq", "dma"):
    extra = " retention" if group == "dma" else ""
    ROUTES["resources", group] = Route(f"acceptance suite-{group}{extra}", group)
for group in ("ipc", "process", "tunnel"):
    extra = " race" if group == "process" else ""
    ROUTES[group, "stress"] = Route(f"acceptance suite-{group} stress{extra}", group, 1, "debug")
ROUTES["memory", "stress"] = Route("acceptance suite-memory memory", "memory", 3, "debug")
ROUTES["platform", "boot"] = Route("acceptance suite-startup", "startup")
ROUTES["platform", "fp"] = Route("shutdown fp", fp=True)
ROUTES["platform", "nofd"] = Route("acceptance suite-startup", "startup", platform="nofd")
ROUTES["platform", "hetero"] = Route("shutdown fp", fp=True, platform="hetero")
ROUTES["platform", "sifive_u"] = Route("acceptance suite-startup", "startup", model="sifive_u")
for name, suite, mode in (("dma-shared", "dma", 32), ("dma-quota", "dma", 64),
                          ("irq-exit", "irq", 256)):
    ROUTES["resources", name] = Route(f"acceptance suite-{suite} mode-{name}", suite, mode)

BASIC = [(group, "basic") for group in ("memory", "ipc", "process", "tunnel", "time")]
BASIC += [("resources", name) for name in ("mmio", "irq", "dma")]
BASIC += [("platform", "fp")]


def select(group, case):
    if group == "all" and case is None:
        return BASIC
    if group == "resources" and case is None:
        return [(group, name) for name in ("mmio", "irq", "dma")]
    key = (group, case or ("fp" if group == "platform" else "basic"))
    if key not in ROUTES:
        raise ValueError(f"unknown acceptance selection: {group} {case or ''}")
    return [key]


def command(key, build=None):
    route = ROUTES[key]
    mode = build or route.build
    # 明确覆盖选择字段，调用者的旧环境不能给某组暗加其他消费者。
    env = {"ERHINO_FORMATION_SOURCES": route.sources,
           "ERHINO_ACCEPTANCE_SUITE": route.suite, "ERHINO_ACCEPTANCE_MODE": str(route.mode),
           "ERHINO_ACCEPTANCE_FP": str(int(route.fp)),
           "ERHINO_ACCEPTANCE_PLATFORM": route.platform,
           "ERHINO_DTB": ""}
    return ["just", "output", "acceptance-" + "-".join(key), "test", "env",
            *(f"{name}={value}" for name, value in env.items()), "just",
            f"MODEL={route.model}", f"MODE={mode}", "run_acceptance"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("group", nargs="?")
    parser.add_argument("case", nargs="?")
    parser.add_argument("--mode", choices=("debug", "release"))
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()
    if not args.group:
        print("Usage: just acceptance <group> [case] [--mode debug|release] [--dry-run]")
        for group in dict.fromkeys(key[0] for key in ROUTES):
            print(f"  {group}: " + ", ".join(case for owner, case in ROUTES if owner == group))
        print("  all: ordinary kernel groups only; no stress or extra platforms")
        return 0
    try:
        commands = [(key, command(key, args.mode)) for key in select(args.group, args.case)]
    except ValueError as error:
        parser.error(str(error))
    for key, argv in commands:
        if args.dry_run:
            print(shlex.join(argv))
            continue
        start = time.monotonic()
        result = subprocess.run(argv, env=os.environ.copy())
        print(f"Acceptance {'/'.join(key)}: exit {result.returncode}, elapsed {time.monotonic()-start:.1f}s", flush=True)
        if result.returncode:
            print("Stopped at the first failure. Inspect its log before selecting adjacent coverage.", file=sys.stderr)
            return result.returncode
    return 0


if __name__ == "__main__":
    sys.exit(main())
