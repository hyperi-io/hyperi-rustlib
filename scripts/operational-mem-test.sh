#!/usr/bin/env bash
# Project:   hyperi-rustlib
# File:      scripts/operational-mem-test.sh
# Purpose:   Cgroup-confined memory backpressure operational test (black box)
# Language:  Bash
#
# License:   BUSL-1.1
# Copyright: (c) 2026 HYPERI PTY LIMITED
#
# Proves the MemoryGuard cap ACTUALLY works in operation, not just in a unit
# test, with a with/without control (the property most often skipped):
#
#   cap=on  under --memory=$MEM  -> survives (exit 0) AND backpressures
#                                   (rejected > 0). Held memory plateaus below
#                                   the limit; the kernel never OOM-kills it.
#   cap=off under --memory=$MEM  -> OOM-killed (non-zero exit, 137). The control
#                                   proves the limit is real and the load
#                                   genuinely over-subscribes -- if cap=off also
#                                   survived, the test would be proving nothing.
#
# This is BLACK BOX: it asserts on the kernel outcome (exit/OOM) and the
# harness's stdout backpressure counters, NOT on rustlib internals.
#
# Usage:  scripts/operational-mem-test.sh [MEM] [DURATION_SECS]
#   MEM            docker --memory value           (default 512m)
#   DURATION_SECS  harness run length for cap=on    (default 15)
#
# Requires docker. Exit 0 = both legs pass.

set -euo pipefail

MEM="${1:-512m}"
DURATION="${2:-15}"
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMAGE="hyperi-rustlib-mem-loadgen:optest"
DOCKERFILE="$REPO_ROOT/scripts/operational/Dockerfile.mem_loadgen"

if ! command -v docker >/dev/null 2>&1; then
    echo "SKIP: docker not available" >&2
    exit 2
fi

# Avoid host credential helpers (e.g. docker-credential-secretservice) that are
# absent in headless/CI contexts -- the public base images need no auth.
# Mirrors rustlib's contract-artefact e2e (docker_empty_creds_json).
DOCKER_CONFIG_DIR="$(mktemp -d)"
printf '{"auths":{}}\n' >"$DOCKER_CONFIG_DIR/config.json"
export DOCKER_CONFIG="$DOCKER_CONFIG_DIR"
trap 'rm -rf "$DOCKER_CONFIG_DIR"' EXIT

echo "== building harness image (memory feature, pure Rust -- light) =="
docker build -f "$DOCKERFILE" -t "$IMAGE" "$REPO_ROOT"

# Over-subscribe: 1 MiB payloads held 4s, far faster than they drain.
COMMON_ENV=(
    -e HARNESS_PAYLOAD_BYTES=1048576
    -e HARNESS_RATE_HZ=50000
    -e HARNESS_HOLD_MS=4000
)

fail() { echo "FAIL: $*" >&2; exit 1; }

echo "== leg 1/2: cap=ON under --memory=$MEM (expect survive + backpressure) =="
on_out="$(docker run --rm --memory="$MEM" --memory-swap="$MEM" \
    "${COMMON_ENV[@]}" -e HARNESS_CAP=on -e HARNESS_DURATION_SECS="$DURATION" \
    "$IMAGE" 2>&1)" && on_rc=0 || on_rc=$?
echo "$on_out"
[ "$on_rc" -eq 0 ] || fail "cap=on was killed (rc=$on_rc) -- backpressure did not hold memory under $MEM"
final_rej="$(printf '%s\n' "$on_out" | sed -n 's/.*rejected=\([0-9]\+\).*/\1/p' | tail -1)"
[ -n "$final_rej" ] && [ "$final_rej" -gt 0 ] \
    || fail "cap=on did not backpressure (rejected=$final_rej); load may not have over-subscribed -- raise rate or lower MEM"
echo "PASS leg 1: survived, rejected=$final_rej (backpressure engaged)"

echo "== leg 2/2: cap=OFF under --memory=$MEM (control: expect OOM-kill) =="
# Cap on a short duration; it should OOM well before this.
off_rc=0
docker run --rm --memory="$MEM" --memory-swap="$MEM" \
    "${COMMON_ENV[@]}" -e HARNESS_CAP=off -e HARNESS_DURATION_SECS=30 \
    "$IMAGE" >/dev/null 2>&1 || off_rc=$?
if [ "$off_rc" -eq 0 ]; then
    fail "cap=off SURVIVED under $MEM -- the control failed: either the limit is not enforced or the load did not over-subscribe. The test proves nothing if the control passes."
fi
echo "PASS leg 2: control OOM-killed (rc=$off_rc)"

echo "== OPERATIONAL MEMORY TEST PASSED (cap bounds memory; control OOMs) =="
