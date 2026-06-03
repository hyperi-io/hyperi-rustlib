#!/usr/bin/env bash
# Project:   hyperi-rustlib
# File:      scripts/dep-dup-check.sh
# Purpose:   Warning-only dependency-duplication report (finding P2 / task 4.3)
# Language:  Bash
#
# License:   BUSL-1.1
# Copyright: (c) 2026 HYPERI PTY LIMITED
#
# Prints duplicated crate versions (`cargo tree -d`). WARNING-ONLY: it never
# exits non-zero, because most duplicates are ecosystem-transitional and
# outside our control (see docs/dependency-duplication.md). CI can run it to
# surface drift without gating the build.

set -uo pipefail

FEATURES="${1:-full}"

echo "== dependency duplication (cargo tree -d --features ${FEATURES}) =="
dupes="$(cargo tree -d --features "${FEATURES}" -e normal 2>/dev/null | grep -E '^[a-z]' || true)"
count="$(printf '%s\n' "${dupes}" | grep -cE '^[a-z]' || true)"

printf '%s\n' "${dupes}"
echo "----"
echo "duplicate crate versions: ${count}"
echo "(warning-only; tracked in docs/dependency-duplication.md -- not a build gate)"

exit 0
