#!/usr/bin/env bash
# 清理泄漏的 qemu-system-riscv64 进程。
#
# 默认只杀「孤儿」qemu（PPID=1）：父进程已退出、无人回收，铁定是残留
# （测试的前台进程 just/bash 退出后，qemu 变 launchd 的孤儿继续空转）。
# 仍在被前台等待的 qemu 有活跃父进程，默认不清理，避免误杀正在跑的测试。
#
# 用法：
#   clean-qemu.sh        列出候选并逐个确认后清理孤儿
#   clean-qemu.sh -y     跳过确认，直接清理孤儿
#   clean-qemu.sh -l     仅列出，不杀
#   clean-qemu.sh -f     强制：连同有父进程的 qemu 一并清理（慎用）

set -euo pipefail

PNAME="qemu-system-riscv64"
ORPHAN_ONLY=1
CONFIRM=1
LIST_ONLY=0

usage() {
    awk 'NR == 1 { next } !/^#/ { exit } { sub(/^# ?/, ""); print }' "$0"
    exit 0
}

while getopts "lfyh" opt; do
    case "$opt" in
        l) LIST_ONLY=1 ;;
        f) ORPHAN_ONLY=0 ;;
        y) CONFIRM=0 ;;
        *) usage ;;
    esac
done

lines=()
while IFS= read -r line; do
    lines+=("$line")
done < <(ps -axo pid=,ppid=,etime=,time=,%cpu=,command= | awk -v p="$PNAME" '$6 == p {print}')

if [[ ${#lines[@]} -eq 0 ]]; then
    echo "no $PNAME processes found"
    exit 0
fi

echo "found ${#lines[@]} $PNAME process(es):"
orphans=()
for line in "${lines[@]}"; do
    read -r pid ppid etime cputime cpu cmd <<< "$line"
    if [[ "$ppid" == "1" ]]; then
        tag=" [orphan -> clean]"
        orphans+=("$pid")
    elif [[ "$ORPHAN_ONLY" == "1" ]]; then
        tag=" [parent ${ppid} active; kept]"
    else
        tag=""
    fi
    printf '  %-7s ppid=%-6s etime=%-9s cpu=%-6s %s%s\n' \
        "$pid" "$ppid" "$etime" "$cpu" "$cmd" "$tag"
done

[[ "$LIST_ONLY" == "1" ]] && exit 0

targets=("${orphans[@]}")
if [[ "$ORPHAN_ONLY" == "0" ]]; then
    targets=()
    for line in "${lines[@]}"; do
        read -r pid _ <<< "$line"
        targets+=("$pid")
    done
fi

if [[ ${#targets[@]} -eq 0 ]]; then
    echo "no orphan qemu to clean; use -f to force-kill all"
    exit 0
fi

if [[ "$CONFIRM" == "1" ]]; then
    printf 'kill %d qemu process(es) [%s]? [y/N] ' "${#targets[@]}" "${targets[*]}"
    read -r ans
    [[ "$ans" == "y" || "$ans" == "Y" ]] || { echo "aborted"; exit 0; }
fi

kill "${targets[@]}"
echo "killed: ${targets[*]}"
