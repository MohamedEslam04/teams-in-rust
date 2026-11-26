# Teams emoticon catalog

`teams-emoticons.tsv` is Teams' own emoticon catalog (ids such as `muscle`, `cwl`,
`1f389_partypopper`, their skin-tone support, names, shortcuts and keywords), as the
Teams web client loads it. `emoji::teams` embeds it.

- Source: `https://statics.teams.cdn.office.net/evergreen-assets/personal-expressions/v1/metadata/<version>/en-us.json`,
  where `<version>` is `emoticonAssetVersion` in the web client's `config-prod-<hash>.js`
  (linked from `https://teams.microsoft.com/v2/`). The file's header names the version and
  the fetch date (first vendored 2026-11-26, version `0f52465a47bf42f299c74a639443f33e`,
  1651 emoticons, 298 with skin tones).
- Refresh: `python3 -I crates/emoji/gen/teams.py` finds the current version and rewrites
  the file. Pass a version to pin one, or a downloaded `en-us.json`.
- Do not edit the file by hand. Stand-in emoji for Teams-only animations (no Unicode emoji)
  live in `TEAMS_ONLY` in the script.
