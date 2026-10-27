#!/usr/bin/env python3
"""清理仓库内可再生成的构建产物，保留运行日志和调查证据。"""

import argparse
from pathlib import Path
import shutil


# 与 Justfile 的产物目录及三个 workspace 的默认 target 目录对应。
BUILD_PATHS = (
    "os/target",
    "shared/target",
    "user/target",
    "artifacts/cargo",
    "artifacts/build",
    "artifacts/qemu",
    "artifacts/initfs",
    "artifacts/initfs.tar",
    "artifacts/boot-package.bin",
)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dry-run", action="store_true", help="list paths without removing them")
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    targets = [root / name for name in BUILD_PATHS]

    # 先验证全部父目录，避免 workspace/artifacts 重定向到其他位置。
    for path in targets:
        if path.parent.resolve() != path.parent:
            parser.error(f"build path has a symlinked parent: {path.relative_to(root)}")

    count = 0
    for path in targets:
        if not path.exists() and not path.is_symlink():
            continue
        print(f"{'Would remove' if args.dry_run else 'Removing'} {path.relative_to(root)}")
        if not args.dry_run:
            if path.is_symlink() or not path.is_dir():
                path.unlink()
            else:
                shutil.rmtree(path)
        count += 1
    print(f"Build cleanup {'preview' if args.dry_run else 'complete'}: {count} paths; evidence retained.")


if __name__ == "__main__":
    main()
