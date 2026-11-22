#!/usr/bin/env bash
# Keeps the Slint UI's layers honest (see crates/ironteams/ui/README.md):
#
#  1. A file only imports from its own layer or a lower one:
#       tokens, stores  <-  atoms  <-  molecules  <-  organisms  <-  pages  <-  app.slint
#  2. Colour literals (#rgb, #rrggbb, #rrggbbaa) live in ui/tokens/ and ui/atoms/. The colours
#     that still sit elsewhere are listed, file by file, in scripts/ui-hex-allowlist.txt; that
#     list may only shrink. Comments are not checked.
#
# Run from anywhere: bash scripts/check-ui-layers.sh. Exits 1 and names each offence.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ui="$repo/crates/ironteams/ui"
allowlist="$repo/scripts/ui-hex-allowlist.txt"
fail=0

# The layer of a file (by its path under ui/), as a number: higher may import lower.
rank() {
    case "$1" in
        tokens/*|stores/*) echo 0 ;;
        atoms/*) echo 1 ;;
        molecules/*) echo 2 ;;
        organisms/*) echo 3 ;;
        pages/*) echo 4 ;;
        app.slint) echo 5 ;;
        *) echo -1 ;;
    esac
}

cd "$ui"
mapfile -t files < <(find . -name '*.slint' -not -path './fonts/*' | sed 's#^\./##' | sort)

# --- 1. imports ---
for f in "${files[@]}"; do
    from=$(rank "$f")
    if [ "$from" -lt 0 ]; then
        echo "layer: $f is outside tokens/, stores/, atoms/, molecules/, organisms/, pages/"
        fail=1
        continue
    fi
    dir=$(dirname "$f")
    while IFS= read -r target; do
        # std-widgets.slint and other library imports have no path of ours.
        case "$target" in std-widgets.slint) continue ;; esac
        resolved=$(realpath -m --relative-to="$ui" "$dir/$target")
        to=$(rank "$resolved")
        if [ "$to" -lt 0 ]; then
            echo "layer: $f imports $target, which is outside the layers"
            fail=1
        elif [ "$to" -gt "$from" ]; then
            echo "layer: $f imports $resolved from a higher layer"
            fail=1
        fi
    done < <(grep -oE '^\s*(import|export)\s*\{[^}]*\}\s*from\s*"[^"]+"' "$f" | sed -E 's/.*"([^"]+)"/\1/')
done

# --- 2. colour literals ---
hex='#[0-9a-fA-F]{8}\b|#[0-9a-fA-F]{6}\b|#[0-9a-fA-F]{3}\b'
for f in "${files[@]}"; do
    case "$f" in tokens/*|atoms/*) continue ;; esac
    # Drop `//` comments (a `//` inside a string is rare in our files), then list the colours.
    while IFS= read -r colour; do
        [ -z "$colour" ] && continue
        if ! grep -qxF "$f $colour" "$allowlist"; then
            echo "colour: $f uses $colour; use a Theme token (ui/tokens/) or an atom variant instead"
            fail=1
        fi
    done < <(sed -E 's#//.*$##' "$f" | grep -oE "$hex" | tr 'A-F' 'a-f' | sort -u)
done

# Allowlist entries that no longer match anything: the list only shrinks.
while IFS= read -r line; do
    case "$line" in ''|'#'*) continue ;; esac
    f=${line% *}
    colour=${line##* }
    used=""
    [ -f "$f" ] && used=$(sed -E 's#//.*$##' "$f" | tr 'A-F' 'a-f' | grep -oE "$colour\b" || true)
    if [ -z "$used" ]; then
        echo "allowlist: '$line' is no longer used; remove it from scripts/ui-hex-allowlist.txt"
        fail=1
    fi
done < "$allowlist"

if [ "$fail" -ne 0 ]; then
    echo "check-ui-layers: FAILED"
    exit 1
fi
echo "check-ui-layers: OK (${#files[@]} files)"
