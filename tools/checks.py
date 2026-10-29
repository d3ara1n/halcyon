#!/usr/bin/env python3
"""选择 package 或 workspace 分面执行静态检查；host 测试始终显式选择本机 target。"""
import argparse
import json
from pathlib import Path
import subprocess
import sys

from qemu_gdb import fresh_logs

ROOT = Path(__file__).resolve().parents[1]
BUILD_STD = ["-Z", "build-std=core,alloc", "-Z", "build-std-features=compiler-builtins-mem"]
FACETS = ("formation-host", "shared-host", "os-host", "kernel", "user-host", "user", "fp")


def packages():
    found = {}
    for workspace in ("os", "shared", "user"):
        metadata = json.loads(subprocess.check_output(
            ["cargo", "metadata", "--no-deps", "--format-version", "1"],
            cwd=ROOT / workspace, text=True))
        for package in metadata["packages"]:
            library = any("lib" in target["kind"] for target in package["targets"])
            found[package["name"]] = (workspace, Path(package["manifest_path"]).parent, library)
    return found


def invocation(action, scope, host, known, filters=()):
    flags = []
    if scope == "formation-host":
        workspace, flags, native = "shared", ["-p", "formation", "--features", "host", "--all-targets"], True
    elif scope in ("shared-host", "os-host", "user-host"):
        workspace, native = scope.split("-")[0], True
        if workspace == "user":
            flags = [arg for name, (owner, _, library) in known.items() if owner == "user" and library for arg in ("-p", name)]
        else:
            flags = ["--workspace"] + (["--exclude", "kernel"] if workspace == "os" else [])
        flags += ["--all-targets"]
    elif scope == "user":
        workspace, flags, native = "user", ["--workspace", "--lib", "--bins", "--exclude", "test_fp"], False
    elif scope in ("kernel", "fp"):
        workspace = "os" if scope == "kernel" else "user"
        name = "kernel" if scope == "kernel" else "test_fp"
        flags, native = ["-p", name, "--bin", name], False
    elif scope in known:
        workspace, _, library = known[scope]
        native = action == "test" or workspace == "shared" or (workspace == "os" and scope != "kernel")
        if action == "test" and (not library or scope == "kernel"):
            raise ValueError("host tests require a library package")
        flags = ["-p", scope]
        if action != "test":
            flags += ["--all-targets"] if native else (["--lib"] if library else ["--bins"])
        if scope == "formation":
            flags += ["--features", "host"]
    else:
        raise ValueError(f"unknown check scope: {scope}")
    if native:
        flags += ["--target", host]
    else:
        flags += BUILD_STD
        if scope in ("fp", "test_fp"):
            flags += ["--target", "platforms/riscv64gc-unknown-erhino-elf.json"]
    if action == "clippy":
        flags += ["--", "-D", "warnings"]
    elif action == "test":
        flags += list(filters)
    return ROOT / workspace, ["cargo", action, *flags]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("check", "clippy", "test"))
    parser.add_argument("scope", nargs="?")
    parser.add_argument("filters", nargs="*")
    args = parser.parse_args()
    if not args.scope:
        print(f"Usage: just {args.action if args.action != 'test' else 'test-host'} <package|scope>")
        print("Scopes: " + ", ".join(FACETS) + ", all (static checks only)")
        return 0
    if args.action == "test" and args.scope in (*FACETS, "all"):
        parser.error("test-host selects one library package")
    if args.action != "test" and args.filters:
        parser.error("filters are only supported by test-host")
    known = packages()
    host = next(line.removeprefix("host: ") for line in subprocess.check_output(["rustc", "-vV"], text=True).splitlines() if line.startswith("host: "))
    try:
        jobs = [(scope, *invocation(args.action, scope, host, known, args.filters)) for scope in (FACETS if args.scope == "all" else (args.scope,))]
    except ValueError as error:
        parser.error(str(error))
    logs = fresh_logs(ROOT / "artifacts" / "checks")
    for scope, cwd, command in jobs:
        log = logs / (scope + ".log")
        print(f"Starting {args.action} {scope}; log: {log}", flush=True)
        with log.open("w") as output:
            result = subprocess.run(command, cwd=cwd, stdout=output, stderr=subprocess.STDOUT)
        print(f"{args.action} {scope}: exit {result.returncode}; log: {log}", flush=True)
        if result.returncode:
            return result.returncode
    return 0


if __name__ == "__main__":
    sys.exit(main())
