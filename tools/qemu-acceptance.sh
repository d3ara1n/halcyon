#!/usr/bin/env bash
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
trap 'rm -f "$log"' EXIT

set +e
"$@" 2>&1 | tee "$log"
status=${PIPESTATUS[0]}
set -e

if [[ "$status" -ne 0 ]]; then
    if ! $allow_timeout || [[ "$status" -ne 124 ]]; then
        echo "QEMU acceptance failed: command exited with status $status." >&2
        exit "$status"
    fi
fi

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
for anchor in "${required[@]}"; do
    if ! grep -Fq "$anchor" "$log"; then
        echo "QEMU acceptance failed: missing anchor: $anchor" >&2
        exit 1
    fi
done

rejected=(
    "acceptance failed"
    "Kernel panicking"
    "Panicking in "
    "Panicking: no information available."
)
for anchor in "${rejected[@]}"; do
    if grep -Fq "$anchor" "$log"; then
        echo "QEMU acceptance failed: observed failure anchor: $anchor" >&2
        exit 1
    fi
done

if [[ "$status" -eq 124 ]]; then
    echo "QEMU acceptance passed before the platform timeout."
fi
