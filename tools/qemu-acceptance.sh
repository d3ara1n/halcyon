#!/usr/bin/env bash
#
# qemu-acceptance.sh — 跑一次 QEMU 验收并按锚点判定成败。
#
# 两种平台结果：
#   默认             显式 reset 成功后 QEMU 退出；任何终态失败锚点立即收割并失败。
#   --allow-timeout   无 shutdown 后端的平台（sifive_u）允许 reset 明确失败；
#                     日志出现失败结果后主动收割，硬 timeout 只兜底真挂死。
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
    "system reset authority checks passed"
    "system reset accepted: action Shutdown, reason Requested"
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
            "failed to start bin/srv_fp: SpawnFailure { error: System(NotSupported), grants: Retained, cleanup_error: None }"
        )
        ;;
    *)
        echo "QEMU acceptance failed: unknown profile: $profile" >&2
        exit 2
        ;;
esac
if $allow_timeout; then
    required+=("system reset failed:")
fi

rejected=(
    "acceptance failed"
    "Kernel panicking"
    "Panicking in "
    "Panicking: no information available."
)
if ! $allow_timeout; then
    rejected+=("system reset failed:")
fi

# reset 成功不返回，由 QEMU 进程退出收束；只有明确失败或 panic 才是可提前
# 收割的终态。判定仍在收割后检查完整 required/rejected 集合。
terminal=(
    "system reset failed:"
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

# 后台跑 + tail 回显：失败锚点必须主动收割，否则 panic 或 reset 返回后的
# 稳态 WFI 不会让 QEMU 自退。TERM 沿 timeout → qemu-throttle → QEMU 链传播。
: > "$log"
"$@" > "$log" 2>&1 &
runner=$!
tail -n +1 -f "$log" &
tailer=$!
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

# 失败即保留现场：无法重现的非确定性失败、锚点缺失或挂死一旦删日志就只能重跑。
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
    echo "QEMU acceptance passed (harvested after explicit reset failure)."
fi
rm -f "$log"
