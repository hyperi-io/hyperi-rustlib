#!/usr/bin/env bash
# Project:   hyperi-rustlib
# File:      scripts/fetch-vector.sh
# Purpose:   Download and cache Vector binary for integration tests
# Language:  Bash
#
# License:   FSL-1.1-ALv2
# Copyright: (c) 2026 HYPERI PTY LIMITED
#
# Usage:
#   ./scripts/fetch-vector.sh              # ensure latest, print binary path
#   VECTOR_VERSION=0.43.0 ./scripts/fetch-vector.sh  # pin specific version
#
# Downloads the latest Vector release only if the cached binary is missing or
# out of date. Prints the absolute path to the vector binary on stdout (last line).
# Status messages go to stderr.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CACHE_DIR="${REPO_ROOT}/.tmp/vector"
ARCH="$(uname -m)"

# Check what we have cached (read version from binary)
cached_version() {
    local bin="${CACHE_DIR}/bin/vector"
    if [[ -x "$bin" ]]; then
        "$bin" --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1
    fi
}

# Resolve the desired version
if [[ -n "${VECTOR_VERSION:-}" ]]; then
    WANT_VERSION="$VECTOR_VERSION"
else
    if command -v gh &>/dev/null; then
        WANT_VERSION=$(gh release list --repo vectordotdev/vector --limit 30 --json tagName \
            --jq '[.[] | select(.tagName | test("^v[0-9]"))][0].tagName' | sed 's/^v//')
    elif command -v jq &>/dev/null; then
        WANT_VERSION=$(curl -fsSL "https://api.github.com/repos/vectordotdev/vector/releases?per_page=30" \
            | jq -r '[.[] | select(.tag_name | test("^v[0-9]"))][0].tag_name' | sed 's/^v//')
    else
        echo "ERROR: need either 'gh' or 'jq' to resolve latest version" >&2
        exit 1
    fi
fi

if [[ -z "$WANT_VERSION" || "$WANT_VERSION" == "null" ]]; then
    echo "ERROR: could not resolve Vector version" >&2
    exit 1
fi

BINARY="${CACHE_DIR}/bin/vector"
HAVE_VERSION=$(cached_version || true)

# If cached binary matches desired version, use it
if [[ "$HAVE_VERSION" == "$WANT_VERSION" ]]; then
    echo "Vector ${WANT_VERSION} already cached" >&2
    echo "$BINARY"
    exit 0
fi

if [[ -n "$HAVE_VERSION" ]]; then
    echo "Updating Vector ${HAVE_VERSION} -> ${WANT_VERSION}" >&2
else
    echo "Downloading Vector ${WANT_VERSION} for ${ARCH}..." >&2
fi

# Clean old cache
rm -rf "${CACHE_DIR:?}/bin"

# Download
mkdir -p "${CACHE_DIR}"
TARBALL_NAME="vector-${WANT_VERSION}-${ARCH}-unknown-linux-gnu.tar.gz"
DOWNLOAD_URL="https://github.com/vectordotdev/vector/releases/download/v${WANT_VERSION}/${TARBALL_NAME}"

curl -fSL --progress-bar -o "${CACHE_DIR}/${TARBALL_NAME}" "$DOWNLOAD_URL"

# Extract — tarball contains vector-{ARCH}-unknown-linux-gnu/bin/vector
echo "Extracting..." >&2
tar xzf "${CACHE_DIR}/${TARBALL_NAME}" -C "${CACHE_DIR}"

EXTRACTED_DIR="${CACHE_DIR}/vector-${ARCH}-unknown-linux-gnu"
if [[ -d "$EXTRACTED_DIR" ]]; then
    mv "${EXTRACTED_DIR}/bin" "${CACHE_DIR}/bin"
    rm -rf "$EXTRACTED_DIR"
fi

# Cleanup tarball
rm -f "${CACHE_DIR}/${TARBALL_NAME}"

# Verify
if [[ ! -x "$BINARY" ]]; then
    echo "ERROR: Vector binary not found at ${BINARY} after extraction" >&2
    exit 1
fi

echo "Vector ${WANT_VERSION} cached at ${BINARY}" >&2
echo "$BINARY"
