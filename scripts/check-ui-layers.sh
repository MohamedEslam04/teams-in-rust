#!/usr/bin/env bash
# Keeps the Slint UI's layers honest (see crates/ironteams/ui/README.md):
#
#  1. A file only imports from its own layer or a lower one:
#       tokens, stores  <-  atoms  <-  molecules  <-  organisms  <-  pages  <-  app.slint
#  2. Colour literals (#rgb, #rgba, #rrggbb, #rrggbbaa) live in ui/tokens/ and nowhere else. Comments
#     are not checked.
#  3. Region colours: the files of an area named in `rule3_areas` read their colours from their
#     `c` input (a `Tokens`), never from the flat colour properties of `Theme` (`Theme.main`,
#     `Theme.fg`, `Theme.dark`, ...), so the nav and the call screen can have colours of their
#     own. Areas: atoms (ui/atoms/ and tokens/motion.slint), molecules, nav (`nav_set`) and call
#     (`call_set`); the other organisms and the pages may read the flat colours. The forbidden
#     names are read from tokens/theme.slint. Non-colour `Theme` properties (`Theme.radius`,
#     `Theme.font-body`, ...) stay readable. Comments are not checked.
#
#  4. Every `Theme.x` and `Fixed.x` read names a property the global declares, so a removed
#     alias (`Theme.sidebar`, `Theme.card`) or a misspelt token fails here, not only in CI.
# Run from anywhere: bash scripts/check-ui-layers.sh. Exits 1 and names each offence.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ui="$repo/crates/ironteams/ui"
fail=0

# The areas rule 3 covers (docs/designs/theme-engine.md).
rule3_areas="atoms molecules nav call"
# The nav: the rail, the sidebar and the chat list.
nav_set=(organisms/app_rail.slint organisms/apps_rail.slint organisms/sidebar.slint
    organisms/lists.slint organisms/chat_list_extras.slint organisms/communities_section.slint
    organisms/web_parity.slint)
# The call screen: every component reachable from CallWindow, CallMini, CallIncoming and
# CallOutgoing that is not an atom or a molecule.
call_set=(pages/call_window.slint organisms/call_audio_menu.slint organisms/call_camera_menu.slint
    organisms/call_feedback.slint organisms/call_incoming.slint organisms/call_mini.slint
    organisms/call_outgoing.slint organisms/call_people.slint organisms/call_prejoin.slint
    organisms/call_stage.slint organisms/call_top_bar.slint organisms/call_transfer.slint
    organisms/video_effects.slint organisms/video_preview.slint)

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
hex='#[0-9a-fA-F]{8}\b|#[0-9a-fA-F]{6}\b|#[0-9a-fA-F]{4}\b|#[0-9a-fA-F]{3}\b'
for f in "${files[@]}"; do
    case "$f" in tokens/*) continue ;; esac
    # Drop `//` comments (a `//` inside a string is rare in our files), then list the colours.
    while IFS= read -r colour; do
        [ -z "$colour" ] && continue
        echo "colour: $f uses $colour; use a Theme token or a Fixed colour (ui/tokens/) instead"
        fail=1
    done < <(sed -E 's#//.*$##' "$f" | grep -oE "$hex" | tr 'A-F' 'a-f' | sort -u)
done

# --- 3. region colours ---
mapfile -t flat < <(sed -nE 's/^\s*out property <(color|bool)> ([a-z-]+): content\..*/\2/p' tokens/theme.slint)
if [ "${#flat[@]}" -eq 0 ]; then
    echo "region: no flat colour aliases found in tokens/theme.slint"
    fail=1
fi
names=$(IFS='|'; echo "${flat[*]}")
for area in $rule3_areas; do
    case "$area" in
        atoms) area_files=(atoms/*.slint tokens/motion.slint) ;;
        molecules) area_files=(molecules/*.slint) ;;
        nav) area_files=("${nav_set[@]}") ;;
        call) area_files=("${call_set[@]}") ;;
        *)
            echo "region: unknown area '$area' in rule3_areas"
            fail=1
            continue
            ;;
    esac
    for f in "${area_files[@]}"; do
        if [ ! -f "$f" ]; then
            echo "region: $f (area $area) does not exist"
            fail=1
            continue
        fi
        while IFS= read -r name; do
            [ -z "$name" ] && continue
            echo "region: $f reads Theme.$name; read it from the c input"
            fail=1
        done < <(sed -E 's#//.*$##' "$f" | grep -oE "Theme\.($names)([^a-z0-9-]|$)" \
            | sed -E 's/^Theme\.([a-z-]+).*/\1/' | sort -u)
    done
done

# --- 4. unknown Theme and Fixed names ---
# A read of a removed alias (`Theme.sidebar`, `Theme.card`) or a misspelt token only fails at
# Slint compile time in CI; catch it here. Names are the properties each global declares.
for global in Theme:tokens/theme.slint Fixed:tokens/fixed.slint; do
    gname=${global%%:*}
    gfile=${global#*:}
    mapfile -t declared < <(sed -nE 's/^\s*(in |out |in-out )?property\s*<[^>]+>\s*([a-z][a-z0-9-]*).*/\2/p' "$gfile" | sort -u)
    if [ "${#declared[@]}" -eq 0 ]; then
        echo "names: no properties found in $gfile"
        fail=1
        continue
    fi
    # A lookup table, not `printf | grep -q`: under pipefail an early grep exit is a false alarm.
    declare -A known=()
    for name in "${declared[@]}"; do known[$name]=1; done
    for f in "${files[@]}"; do
        while IFS= read -r name; do
            [ -z "$name" ] && continue
            if [ -z "${known[$name]:-}" ]; then
                echo "names: $f reads $gname.$name, which $gfile does not declare"
                fail=1
            fi
        done < <(sed -E 's#//.*$##' "$f" | grep -oE "\b$gname\.[a-z][a-z0-9-]*" \
            | sed -E "s/^$gname\.//" | sort -u)
    done
    unset known
done

if [ "$fail" -ne 0 ]; then
    echo "check-ui-layers: FAILED"
    exit 1
fi
echo "check-ui-layers: OK (${#files[@]} files)"
