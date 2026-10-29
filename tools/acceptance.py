#!/usr/bin/env python3
"""按领域选择验收；无参数只列目录，完整回归与故障观察必须显式选择。"""
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
    observer: str = "normal"
    services: bool = False
    fp: bool = False
    model: str = "virt"
    platform: str = ""


ROUTES = {}
for group in ("memory", "ipc", "process", "transport", "time"):
    ROUTES[group, "basic"] = Route(f"acceptance suite-{group}", group)
for group in ("mmio", "irq", "dma"):
    extra = " retention" if group == "dma" else ""
    ROUTES["resources", group] = Route(f"acceptance suite-{group}{extra}", group)
for group in ("ipc", "process", "transport"):
    extra = " race" if group == "process" else ""
    ROUTES[group, "stress"] = Route(f"acceptance suite-{group} stress{extra}", group, 1, "debug")
ROUTES["memory", "stress"] = Route("acceptance suite-memory memory", "memory", 3, "debug")
ROUTES["memory", "fault"] = Route("acceptance suite-memory memory", "memory", 3, "debug", "vm-fault")
ROUTES["memory", "repair"] = Route("acceptance suite-memory memory", "memory", 3, "debug", "vm-repair")
ROUTES["services", "basic"] = Route("system acceptance suite-startup services-check", "startup", services=True)
ROUTES["platform", "boot"] = Route("acceptance suite-startup", "startup")
ROUTES["platform", "fp"] = Route("shutdown fp", fp=True)
ROUTES["platform", "nofd"] = Route("acceptance suite-startup", "startup", platform="nofd")
ROUTES["platform", "hetero"] = Route("shutdown fp", fp=True, platform="hetero")
ROUTES["platform", "sifive_u"] = Route("acceptance suite-startup", "startup", model="sifive_u")
for case in ("boot-failure", "ipi-failure", "platform-funding"):
    ROUTES["platform", case] = Route("acceptance suite-startup", "startup", build="debug", observer=case)
ROUTES["platform", "clock"] = Route("acceptance suite-irq mode-irq-exit", "irq", 256, "debug", "clock")
for name, suite, mode in (("mmio-publication", "mmio", 4), ("mmio-forced", "mmio", 8),
                          ("dma-races", "dma", 16), ("dma-shared", "dma", 32),
                          ("dma-quota", "dma", 64), ("dma-install-failure", "dma", 128),
                          ("irq-exit", "irq", 256)):
    ROUTES["resources", name] = Route(f"acceptance suite-{suite} mode-{name}", suite, mode, "debug", name)
for case in ("dma-retention", "dma-ipi-retention"):
    ROUTES["resources", case] = Route("acceptance suite-dma retention", "dma", build="debug", observer=case)

BASIC = [(group, "basic") for group in ("memory", "ipc", "process", "transport", "time")]
BASIC += [("resources", name) for name in ("mmio", "irq", "dma")]
BASIC += [("services", "basic"), ("platform", "fp")]


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
    if route.observer != "normal" and mode != "debug":
        raise ValueError("external observers require debug ELF")
    # 明确覆盖所有选择字段，调用者的旧环境不能给某组暗加其他消费者。
    env = {"ERHINO_FORMATION_SOURCES": route.sources,
           "ERHINO_ACCEPTANCE_SUITE": route.suite, "ERHINO_ACCEPTANCE_MODE": str(route.mode),
           "ERHINO_ACCEPTANCE_SERVICES": str(int(route.services)),
           "ERHINO_ACCEPTANCE_FP": str(int(route.fp)),
           "ERHINO_ACCEPTANCE_PLATFORM": route.platform,
           "ERHINO_DTB": ""}
    if route.observer not in ("normal", "vm-fault", "boot-failure", "ipi-failure", "platform-funding", "vm-repair"):
        env["THROTTLE"] = "100"
    return ["just", "output", "acceptance-" + "-".join(key), "test", "env",
            *(f"{name}={value}" for name, value in env.items()), "just",
            f"MODEL={route.model}", f"MODE={mode}", "run_acceptance", route.observer]


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
        print("  all: ordinary groups only; no stress, fault injection or extra platforms")
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
