#!/usr/bin/env bash
#
# qemu-acceptance.sh — 跑一次 QEMU 验收并按锚点判定成败。
#
# 两种收束模式：
#   默认         平台自停机（SRST 让 QEMU 退出），前台直跑到进程结束。
#   --allow-timeout  无 shutdown 设备的平台（sifive_u）：QEMU 永不自退。
#                日志出现终态锚点即主动收割（正常轮次不必空等硬上限），
#                真挂死则由调用方的 timeout 兜底（status 124 视为可接受）。
set -u -o pipefail

allow_timeout=false
profile="${QEMU_ACCEPTANCE_PROFILE:-common}"
if [[ "${1:-}" == "--allow-timeout" ]]; then
    allow_timeout=true
    shift
fi
if [[ "${1:-}" != "--" ]]; then
    echo "Usage: $0 [--allow-timeout] -- <qemu command...>" >&2
    exit 2
fi
shift
if [[ "$#" -eq 0 ]]; then
    echo "QEMU acceptance command is missing." >&2
    exit 2
fi

mkdir -p artifacts
log="artifacts/.qemu-acceptance-$$.log"

required=(
    "drain minimum-budget acceptance passed"
    "race matrix acceptance passed: 10/10 scenarios passed"
    "acceptance domain collected"
    "all services supervised to completion"
    "peer closed observed"
    "pm delegated domain confirmed Dead"
    "system quiescent (no waker)"
)
case "$profile" in
    common) ;;
    hetero)
        required+=(
            "domain 0 [Base64] -> harts [0]"
            "domain 1 [Base64+D64] -> harts [1, 2, 3]"
            "fp verification passed"
        )
        ;;
    nofd)
        required+=(
            "domain 0 [Base64] -> harts [0, 1, 2, 3]"
            "failed to start bin/srv_fp: System(NotSupported)"
        )
        ;;
    *)
        echo "QEMU acceptance failed: unknown profile: $profile" >&2
        exit 2
        ;;
esac

rejected=(
    "acceptance failed"
    "Kernel panicking"
    "Panicking in "
    "Panicking: no information available."
)

# 终态锚点：出现任一即本轮不会再产生新信息（正常静默停机，或收束型
# panic），可以收割 QEMU；判定仍走完整锚点集。
terminal=(
    "system quiescent (no waker)"
    "acceptance failed"
    "Kernel panicking"
    "Panicking: no information available."
)

log_has() {
    grep -Fq "$1" "$log" 2>/dev/null
}

log_has_any() {
    local anchor
    for anchor in "$@"; do
        log_has "$anchor" && return 0
    done
    return 1
}

if $allow_timeout; then
    # 后台跑 + tail 回显：主循环轮询终态锚点，出现即收割。TERM 沿
    # timeout → qemu-throttle → QEMU 链传播（各层自带清理）。
    : > "$log"
    "$@" > "$log" 2>&1 &
    runner=$!
    tail -n +1 -f "$log" &
    tailer=$!
    # 中途中断（Ctrl-C / 上层 kill）时两个子进程都要收，否则 QEMU 泄为孤儿。
    trap 'kill "$runner" "$tailer" 2>/dev/null || true' EXIT INT TERM
    status=0
    while :; do
        if ! kill -0 "$runner" 2>/dev/null; then
            wait "$runner"
            status=$?
            break
        fi
        if log_has_any "${terminal[@]}"; then
            kill "$runner" 2>/dev/null || true
            wait "$runner" 2>/dev/null || true
            status=124
            break
        fi
        sleep 0.2
    done
    sleep 0.2
    kill "$tailer" 2>/dev/null || true
    wait "$tailer" 2>/dev/null || true
    trap - EXIT INT TERM
else
    set +e
    "$@" 2>&1 | tee "$log"
    status=${PIPESTATUS[0]}
    set -e
fi

# 失败即保留现场：无法重现的非确定性失败（提前停机、锚点缺失、
# 挂死）一旦删日志就只能重跑碍运气。成功才清理。
keep_log() {
    local reason="$1"
    local kept="artifacts/failed-acceptance-$(date +%Y%m%d-%H%M%S)-$$.log"
    if mv "$log" "$kept" 2>/dev/null; then
        echo "QEMU acceptance failure log kept: $kept ($reason)" >&2
    fi
}

if [[ "$status" -ne 0 ]]; then
    if ! $allow_timeout || [[ "$status" -ne 124 ]]; then
        echo "QEMU acceptance failed: command exited with status $status." >&2
        keep_log "exit status $status"
        exit "$status"
    fi
fi

for anchor in "${required[@]}"; do
    if ! log_has "$anchor"; then
        echo "QEMU acceptance failed: missing anchor: $anchor" >&2
        keep_log "missing anchor: $anchor"
        exit 1
    fi
done

for anchor in "${rejected[@]}"; do
    if log_has "$anchor"; then
        echo "QEMU acceptance failed: observed failure anchor: $anchor" >&2
        keep_log "failure anchor: $anchor"
        exit 1
    fi
done

if [[ "$status" -eq 124 ]]; then
    echo "QEMU acceptance passed (harvested at terminal anchor)."
fi
rm -f "$log"
