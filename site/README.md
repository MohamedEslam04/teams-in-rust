# IronTeams website

The landing, download and docs site, published to GitHub Pages at
<https://mohamedeslam04.github.io/teams-in-rust/> by `.github/workflows/pages.yml`.

Plain HTML, CSS and vanilla JavaScript. No build step, no framework, no analytics, no third-party
CDN or font. The only request to another origin is the GitHub API call that lists the downloads.

## Pages

| File | Content |
|---|---|
| `index.html` | Landing: pitch, memory comparison, features, the **Download** chooser, disclaimer |
| `install.html` | Install guides per format, updates, uninstall, build from source |
| `getting-started.html` | First start and sign-in |
| `platforms.html` | Platform table and requirements (from the README) |
| `troubleshooting.html` | Problem report, environment variables, common problems |
| `faq.html` | Short answers |
| `404.html` | Not-found page (uses absolute `/teams-in-rust/` paths, since it is served at any URL) |

The header and footer are repeated in each page. Edit all of them when you change navigation.
Paths are relative, so the site works from the `/teams-in-rust/` sub-path and from a custom domain
(except `404.html`, which names the sub-path).

## How the downloads work

`js/releases.js` is the logic and has no DOM access (it also runs in Node);
`js/download.js` draws it; `js/copy.js` adds the Copy buttons to code blocks.

1. **Detect the system.** `detectOS` reads `navigator.userAgentData.platform` when the browser has
   it, else `navigator.userAgent`. Android and iOS are told that IronTeams is a desktop app. A Linux
   distribution cannot be detected, so Linux shows every format with the AppImage recommended. On
   macOS, `userAgentData.getHighEntropyValues(['architecture'])` (Chromium only) picks Apple Silicon
   or Intel; Safari and Firefox cannot say, so both packages are listed and the hero button offers
   Apple Silicon.
2. **Read the releases live.** `GET https://api.github.com/repos/MohamedEslam04/teams-in-rust/releases`
   without a token. The trimmed answer is cached in `sessionStorage` for 15 minutes (the limit is 60
   requests per hour per IP). If the call fails, an older cached answer is used when there is one;
   otherwise the page shows a link to the GitHub releases page. Drafts are ignored, and
   pre-releases only show behind the **Show beta releases** checkbox.
3. **Map files by name.** `ASSETS` in `releases.js` lists a pattern per download. The names come
   from `.github/workflows/release.yml` (Velopack names them per channel):

   | Download | File name pattern |
   |---|---|
   | AppImage | `IronTeams.AppImage` |
   | RPM | `ironteams-*.x86_64.rpm` |
   | Flatpak | `ironteams.flatpak` |
   | tar.gz | `IronTeams-*-linux-x86_64.tar.gz` |
   | Windows Setup | `IronTeams-win-Setup.exe` |
   | Windows portable | `IronTeams-win-Portable.zip` (else `IronTeams-*-windows-x86_64-portable.zip`) |
   | macOS Apple Silicon | `IronTeams-osx-arm64-Setup.pkg` |
   | macOS Intel | `IronTeams-osx-x64-Setup.pkg` |

4. **Newest release that has the file.** A tag push releases Linux only; Windows and macOS files are
   added later by a manual run, so the newest release may lack them. Every download is looked up
   separately: the newest release that has it, with its version, size, date and SHA-256 (the API's
   `digest`). When that release is older than the newest one, the row says so. When no release has
   the file, the row says "Not available yet" and has no link. Download links are only used when
   they start with `https://github.com/MohamedEslam04/teams-in-rust/releases/download/`.

To add a download, add an entry to `ASSETS`, an entry to the panel it belongs to (`os`), and an
install section in `install.html`.

## Try it locally

```sh
python3 -m http.server -d site 8765
# open http://127.0.0.1:8765/
```

`node --check site/js/*.js` checks the syntax. To test the mapping against the live API:

```sh
curl -s 'https://api.github.com/repos/MohamedEslam04/teams-in-rust/releases?per_page=50' > /tmp/releases.json
node -e 'const R=require("./site/js/releases.js");const p=R.pick(R.normalize(require("/tmp/releases.json")),false);for(const[k,v]of Object.entries(p.items))console.log(k,v&&v.asset.name,v&&v.version)'
```

## Design notes

- System font stacks only, light and dark follow `prefers-color-scheme` (no toggle), motion is one
  bar-growth animation that stops under `prefers-reduced-motion`.
- The site does not use Microsoft or Teams logos or branding. The icon is the project's own
  (`assets/icon.svg`, a copy of `packaging/io.github.mohamedeslam04.ironteams.svg`).
- Facts come from the repository README and `packaging/`. When the README changes (features,
  platform table, install steps), update the matching page.
- The screenshots in `docs/screenshots/` are not used: they show the old name "kelma" in the window,
  and the PNGs on `dev` have a damaged text chunk from the rename.

Docs synced to commit 7cd34f85 ("Merge feat/channel-post-layout: channel posts as full-width cards with a title, headings and lists like Teams", 2026-11-24)
