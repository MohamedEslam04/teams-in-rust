#!/usr/bin/env bash
# Builds the Velopack release of one system and architecture (the packages that update
# themselves): Setup.exe on Windows, the .pkg on macOS, the AppImage on Linux, plus the full and
# delta .nupkg and the releases.<channel>.json feed the app reads from the GitHub releases.
#
#   velopack.sh <channel> <packDir> <mainExe> <icon> [extra vpk pack args...]
#
# Env: APP_NAME (the pack id), VERSION, PRERELEASE (true for a beta tag), GH_TOKEN (read-only is
# enough), GITHUB_REPOSITORY. The newest earlier release of the channel is downloaded first so
# `vpk pack` can build the delta from it; the files that belong in this release end up in
# velopack-out/. Needs `vpk` (dotnet tool, same version as the velopack crate).
set -euo pipefail
channel=${1:?channel}
pack_dir=${2:?packDir}
main_exe=${3:?mainExe}
icon=${4:?icon}
shift 4

repo="https://github.com/$GITHUB_REPOSITORY"
mkdir -p releases velopack-out
pre=()
[[ "${PRERELEASE:-false}" == true ]] && pre=(--pre)
# The first release of a channel has nothing to build a delta from.
vpk download github --repoUrl "$repo" --channel "$channel" --outputDir releases --token "$GH_TOKEN" ${pre[@]+"${pre[@]}"} \
  || echo "::warning::no earlier $channel release to build a delta from; this one is full only"
ls releases > before.txt

vpk pack --packId "$APP_NAME" --packVersion "$VERSION" --packDir "$pack_dir" --mainExe "$main_exe" \
  --packTitle "$APP_NAME" --icon "$icon" --channel "$channel" --outputDir releases ${1+"$@"}

# What this release adds: the new packages and installers, and the feeds pack rewrote. The
# earlier release's full package stays where it is.
for f in releases/*; do
  n=$(basename "$f")
  case "$n" in
    assets.*.json) continue ;;
    releases.*.json | RELEASES*) cp "$f" velopack-out/ ;;
    *) grep -qxF "$n" before.txt || cp "$f" velopack-out/ ;;
  esac
done
ls -la velopack-out
