#!/usr/bin/env bash
# Removes the workspace crates' build output from a cargo target profile directory before CI caches
# it (for the package jobs that do not use Swatinem/rust-cache, which does the same). Every build
# compiles the workspace crates again anyway; only the third-party dependencies are worth keeping,
# and dropping the rest (the ironteams binary, the Slint-generated code) keeps the cache small.
#
#   prune-target.sh <target/release> [Cargo.lock]
# Needs awk or python3 (the CI containers have one or the other).
set -euo pipefail
dir=${1:?target profile dir, e.g. target/release}
lock=${2:-Cargo.lock}
[[ -d "$dir" ]] || { echo "no $dir, nothing to prune"; exit 0; }
du -sh "$dir" 2>/dev/null || true
# Workspace crates are the [[package]] entries without a `source`.
if command -v awk >/dev/null; then
  names=$(awk 'BEGIN { RS = "" } /^\[\[package\]\]/ && !/\nsource = / {
    if (match($0, /\nname = "[^"]+"/)) print substr($0, RSTART + 9, RLENGTH - 10) }' "$lock")
else
  names=$(python3 -I -c '
import re, sys
for b in open(sys.argv[1], encoding="utf-8").read().split("\n\n"):
    m = re.search(r"\nname = \"([^\"]+)\"", b)
    if b.lstrip("\n").startswith("[[package]]") and "\nsource = " not in b and m:
        print(m.group(1))
' "$lock")
fi
for name in $names; do
  u=${name//-/_}
  rm -rf "${dir:?}/$name" "$dir/$name.d" "$dir"/deps/"$u"-* "$dir"/deps/lib"$u"-* \
    "$dir"/.fingerprint/"$name"-* "$dir"/build/"$name"-*
done
rm -rf "$dir/incremental"
du -sh "$dir" 2>/dev/null || true
