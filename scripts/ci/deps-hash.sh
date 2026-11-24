#!/usr/bin/env bash
# Prints a short hash of the third-party dependencies in Cargo.lock (every [[package]] with a
# `source`, so crates.io and git crates), for the keys of the cargo target caches in CI.
# The workspace crates have no `source`: a version bump (chore(release)) leaves the hash alone,
# so a tag restores the cache that dev saved for the same dependencies.
#
#   deps-hash.sh [Cargo.lock]
# Needs awk or python3 (the CI containers have one or the other).
set -euo pipefail
lock=${1:-Cargo.lock}
if command -v awk >/dev/null; then
  awk 'BEGIN { RS = "" } /\nsource = / { print; print "" }' "$lock" | sha256sum | cut -c1-16
else
  python3 -I -c '
import sys
blocks = open(sys.argv[1], encoding="utf-8").read().split("\n\n")
for b in blocks:
    if "\nsource = " in b:
        print(b.strip("\n")); print()
' "$lock" | sha256sum | cut -c1-16
fi
