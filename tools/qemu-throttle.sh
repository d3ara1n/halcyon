#!/usr/bin/env bash
#
# qemu-throttle.sh — 用 SIGSTOP/SIGCONT 占空比节流 QEMU 进程的 CPU 占用。
#
# 背景：guest 跑飞/panic 时 QEMU 会满核空转烧 CPU（实测单核 100%、4 核 ~300%）。
# QEMU 内部没有任何 CPU 限制参数（-icount 对死循环无效、nice 只降优先级不降
# 占用），唯一可靠手段是操作系统层把 QEMU 进程周期性 STOP/CONT、冻结其执行。
# 对 guest 完全透明（感知为「时间暂停」，跑飞场景无副作用）；纯 POSIX 信号，
# 零代码入侵。详见 Justfile 的 THROTTLE 变量。
#
# 用法：tools/qemu-throttle.sh <百分比> <QEMU 命令行...>
#   <百分比> 1-100：100 = 全速直跑（不节流）；50 = 半速；其余按比例。
#
# 节流周期固定 0.5s：跑飞兜底不需要细粒度，粗反而平滑（gdb 卡顿更轻）。
# 退出/被杀时保证先 SIGCONT 解冻再清理，绝不把 QEMU 留在 STOP 态。
set -uo pipefail

THROTTLE="${1:?usage: qemu-throttle.sh <1-100> <qemu cmd...>}"
shift

# 参数校验：仅接受 1-100 整数
case "$THROTTLE" in
    *[!0-9]*|'')
        echo "qemu-throttle: invalid throttle percentage '$THROTTLE' (expect 1-100)" >&2
        exit 2
        ;;
esac
if [ "$THROTTLE" -lt 1 ] || [ "$THROTTLE" -gt 100 ]; then
    echo "qemu-throttle: throttle percentage must be 1-100, got $THROTTLE" >&2
    exit 2
fi

# 100 = 全速：直接前台执行，QEMU 退出码与终端交互原样透传
if [ "$THROTTLE" -eq 100 ]; then
    exec "$@"
fi

# 节流路径：后台跑 QEMU，按占空比 STOP/CONT
"$@" &
QPID=$!

cleanup() {
    local code=$?
    # 先解冻再终止，避免 QEMU 残留 STOP 态
    kill -CONT "$QPID" 2>/dev/null || true
    kill "$QPID" 2>/dev/null || true
    wait "$QPID" 2>/dev/null || true
    exit "$code"
}
trap cleanup EXIT INT TERM

# 0.5s 周期：每轮跑 WORK 秒、停 SLEEP 秒
WORK=$(awk "BEGIN{printf \"%.3f\", 0.5*$THROTTLE/100}")
SLEEP=$(awk "BEGIN{printf \"%.3f\", 0.5-$WORK}")

while kill -0 "$QPID" 2>/dev/null; do
    kill -CONT "$QPID" 2>/dev/null || break
    sleep "$WORK"
    kill -STOP "$QPID" 2>/dev/null || break
    sleep "$SLEEP"
done

wait "$QPID"
exit $?
