#!/usr/bin/env bash
# Project:   hyperi-rustlib
# File:      scripts/operational-cpu-test.sh
# Purpose:   Cgroup-confined CPU oversubscription operational test (black box)
# Language:  Bash
#
# License:   BUSL-1.1
# Copyright: (c) 2026 HYPERI PTY LIMITED
#
# Proves the adaptive worker pool PARKS idle workers in operation (not just in
# a unit test), with a with/without control -- the property most often skipped:
#
#   cap=on  under --cpus=$CPUS -> burns ~only the burst's CPU, then parks while
#                                 idle. NOT meaningfully CPU-throttled by the
#                                 cgroup, because parked workers consume nothing.
#   cap=off under --cpus=$CPUS -> the control: over-subscribed OS threads
#                                 busy-spin on an empty queue, pegging the whole
#                                 cgroup CPU quota for the full window and
#                                 getting THROTTLED (throttled_usec > 0). It
#                                 burns many times the CPU-seconds of cap=on.
#                                 If cap=off did not, the test proves nothing.
#
# BLACK BOX: asserts on the CPU-seconds the process actually burned (from
# /proc/self/stat) and the cgroup's throttled_usec -- NOT on rustlib internals.
#
# Usage:  scripts/operational-cpu-test.sh [CPUS] [IDLE_MS]
#   CPUS     docker --cpus value             (default 0.5)
#   IDLE_MS  idle window after the burst (ms) (default 6000)
#
# Requires docker. Exit 0 = both legs pass.

set -euo pipefail

CPUS="${1:-0.5}"
IDLE_MS="${2:-6000}"
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMAGE="hyperi-rustlib-cpu-loadgen:optest"
DOCKERFILE="$REPO_ROOT/scripts/operational/Dockerfile.cpu_loadgen"

if ! command -v docker >/dev/null 2>&1; then
    echo "SKIP: docker not available" >&2
    exit 2
fi

# Avoid host credential helpers absent in headless/CI contexts -- the public
# base images need no auth. Mirrors the memory operational test.
DOCKER_CONFIG_DIR="$(mktemp -d)"
printf '{"auths":{}}\n' >"$DOCKER_CONFIG_DIR/config.json"
export DOCKER_CONFIG="$DOCKER_CONFIG_DIR"
trap 'rm -rf "$DOCKER_CONFIG_DIR"' EXIT

echo "== building harness image (worker feature, pure Rust -- light) =="
docker build -f "$DOCKERFILE" -t "$IMAGE" "$REPO_ROOT"

# Burst is deliberately small relative to the idle window so the parked-idle
# property dominates the measurement.
COMMON_ENV=(
    -e HARNESS_BURST_TASKS=64
    -e HARNESS_TASK_ITERS=2000000
    -e HARNESS_IDLE_MS="$IDLE_MS"
)

fail() { echo "FAIL: $*" >&2; exit 1; }

# Extract `key=<number>` (int or float) from harness stdout, last occurrence.
extract() { printf '%s\n' "$1" | sed -n "s/.*$2=\\([0-9.]\\+\\).*/\\1/p" | tail -1; }

echo "== leg 1/2: cap=ON under --cpus=$CPUS (expect parked idle, low CPU) =="
on_out="$(docker run --rm --cpus="$CPUS" \
    "${COMMON_ENV[@]}" -e HARNESS_CAP=on "$IMAGE" 2>&1)"
echo "$on_out"
on_cpu="$(extract "$on_out" cpu_seconds)"
[ -n "$on_cpu" ] || fail "could not parse cap=on cpu_seconds"

echo "== leg 2/2: cap=OFF under --cpus=$CPUS (control: expect spin + throttle) =="
off_out="$(docker run --rm --cpus="$CPUS" \
    "${COMMON_ENV[@]}" -e HARNESS_CAP=off "$IMAGE" 2>&1)"
echo "$off_out"
off_cpu="$(extract "$off_out" cpu_seconds)"
off_throttled="$(extract "$off_out" throttled_usec)"
[ -n "$off_cpu" ] || fail "could not parse cap=off cpu_seconds"
[ -n "$off_throttled" ] || off_throttled=0

# Property 1: the control burns at least 2x the CPU-seconds of the parked pool.
ratio_ok="$(awk -v on="$on_cpu" -v off="$off_cpu" 'BEGIN { print (off >= 2 * on) ? 1 : 0 }')"
[ "$ratio_ok" = "1" ] \
    || fail "cap=off CPU ($off_cpu s) not >= 2x cap=on ($on_cpu s) -- pool did not park idle, or control did not over-subscribe"

# Property 2 (control sanity): the spinners were CPU-throttled by the cgroup.
# This proves the --cpus cap is real and the control genuinely over-subscribes.
throttled_ok="$(awk -v t="$off_throttled" 'BEGIN { print (t > 0) ? 1 : 0 }')"
[ "$throttled_ok" = "1" ] \
    || fail "cap=off was not CPU-throttled (throttled_usec=$off_throttled) -- the cgroup cap may not be enforced; the test proves nothing if the control is not throttled"

echo "PASS: cap=on burned ${on_cpu}s, cap=off burned ${off_cpu}s (throttled ${off_throttled}us)"
echo "== OPERATIONAL CPU TEST PASSED (pool parks idle; busy-spin control throttles) =="
