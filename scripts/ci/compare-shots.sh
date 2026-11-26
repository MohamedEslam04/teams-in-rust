#!/usr/bin/env bash
# Compares two directories of screenshots pixel for pixel (ui-compare.yml: the pictures of the
# base commit against the pictures of the pull request).
#
#   compare-shots.sh <base-dir> <head-dir> <diff-dir>
#
# Prints every picture that differs, is missing from one side or has another size, and writes a
# side-by-side picture of each (base, differing pixels in red, head) to <diff-dir>. Exits 0 only
# when both directories hold the same PNG files with identical pixels; 1 on any difference; 2
# when a directory is missing or holds no pictures. Needs ImageMagick (6 or 7), which the Ubuntu
# runners have.
set -euo pipefail

base=${1:?base directory of PNGs}
head=${2:?head directory of PNGs}
out=${3:?output directory for the side-by-side pictures}

for d in "$base" "$head"; do
  [[ -d "$d" ]] || { echo "error: no directory $d" >&2; exit 2; }
done

# ImageMagick 7 has one `magick` command; 6 has `compare`, `convert` and `identify`.
if command -v magick >/dev/null; then
  im_compare() { magick compare "$@"; }
  im_convert() { magick "$@"; }
  im_size() { magick identify -format '%wx%h' "$1"; }
else
  im_compare() { compare "$@"; }
  im_convert() { convert "$@"; }
  im_size() { identify -format '%wx%h' "$1"; }
fi

shopt -s nullglob
names=()
for f in "$base"/*.png "$head"/*.png; do names+=("${f##*/}"); done
if ((${#names[@]} == 0)); then
  echo "error: no PNG files in $base or $head" >&2
  exit 2
fi
mapfile -t names < <(printf '%s\n' "${names[@]}" | sort -u)

mkdir -p "$out"
same=0 differ=0 missing=0
for n in "${names[@]}"; do
  a="$base/$n" b="$head/$n"
  if [[ ! -f "$a" ]]; then
    echo "NEW      $n (not in the base pictures)"
    missing=$((missing + 1))
    continue
  fi
  if [[ ! -f "$b" ]]; then
    echo "MISSING  $n (not in the head pictures)"
    missing=$((missing + 1))
    continue
  fi
  stem=${n%.png}
  if [[ $(im_size "$a") != "$(im_size "$b")" ]]; then
    echo "SIZE     $n: base $(im_size "$a"), head $(im_size "$b")"
    im_convert "$a" "$b" -background magenta -gravity NorthWest +append "$out/$stem.png"
    differ=$((differ + 1))
    continue
  fi
  # `compare` exits 0 for identical pictures, 1 for different ones and 2 for an error; -metric AE
  # prints the number of differing pixels (to stderr) and -fuzz 0 allows no tolerance.
  rc=0
  count=$(im_compare -metric AE -fuzz 0 -highlight-color red -lowlight-color none \
    "$a" "$b" "$out/.diff.png" 2>&1) || rc=$?
  if ((rc == 0)); then
    same=$((same + 1))
    continue
  fi
  if ((rc != 1)); then
    echo "ERROR    $n: compare failed ($count)" >&2
    differ=$((differ + 1))
    continue
  fi
  echo "DIFFERS  $n: ${count%% *} pixels"
  im_convert "$a" "$out/.diff.png" "$b" -background gray -gravity NorthWest +append "$out/$stem.png"
  differ=$((differ + 1))
done
rm -f "$out/.diff.png"

echo "$same identical, $differ different, $missing missing or new, of ${#names[@]} pictures"
((differ == 0 && missing == 0))
