/* Pure logic for the download section: OS detection, release filtering and asset mapping.
   No DOM access, so it also runs in Node (see site/README.md). Loaded as a classic script
   (window.IronReleases) or required (module.exports). */
(function (root, factory) {
  if (typeof module === 'object' && module.exports) module.exports = factory();
  else root.IronReleases = factory();
})(typeof self !== 'undefined' ? self : this, function () {
  'use strict';

  var REPO = 'MohamedEslam04/teams-in-rust';
  var API_URL = 'https://api.github.com/repos/' + REPO + '/releases?per_page=50';
  var RELEASES_PAGE = 'https://github.com/' + REPO + '/releases';
  var DOWNLOAD_PREFIX = 'https://github.com/' + REPO + '/releases/download/';

  /* Asset catalogue. `match` runs against the file name in a release; the first pattern of
     `match` that finds a file wins. Names come from .github/workflows/release.yml (Velopack
     names per channel: IronTeams.AppImage, IronTeams-win-Setup.exe, IronTeams-osx-arm64-Setup.pkg …). */
  var ASSETS = {
    'linux-appimage': {
      os: 'linux', format: 'AppImage', recommended: true, installId: 'appimage',
      blurb: 'One file, no install step. Updates itself.',
      match: [/^IronTeams\.AppImage$/]
    },
    'linux-rpm': {
      os: 'linux', format: 'RPM', installId: 'rpm',
      blurb: 'For Fedora. Updated by your package manager.',
      match: [/^ironteams-[^/]+\.x86_64\.rpm$/, /^ironteams-(?!.*\.src\.rpm$)[^/]+\.rpm$/]
    },
    'linux-flatpak': {
      os: 'linux', format: 'Flatpak', installId: 'flatpak',
      blurb: 'Sandboxed bundle. Needs the Freedesktop 26.08 runtime.',
      match: [/^ironteams\.flatpak$/]
    },
    'linux-tar': {
      os: 'linux', format: 'tar.gz', installId: 'tarball',
      blurb: 'The plain binary with its desktop file and icon. Does not update itself.',
      match: [/^IronTeams-[^/]+-linux-x86_64\.tar\.gz$/]
    },
    'windows-setup': {
      os: 'windows', format: 'Setup.exe', recommended: true, installId: 'windows',
      blurb: 'Per-user install, no administrator needed. Updates itself. Not signed yet.',
      match: [/^IronTeams-win-Setup\.exe$/]
    },
    'windows-portable': {
      os: 'windows', format: 'Portable zip', installId: 'windows',
      blurb: 'Unzip anywhere and run.',
      match: [/^IronTeams-win-Portable\.zip$/, /^IronTeams-[^/]+-windows-x86_64-portable\.zip$/]
    },
    'macos-arm64': {
      os: 'macos', format: '.pkg for Apple Silicon', arch: 'arm64', installId: 'macos',
      blurb: 'Macs with an M-series chip. Updates itself. Not signed yet.',
      match: [/^IronTeams-osx-arm64-Setup\.pkg$/]
    },
    'macos-x64': {
      os: 'macos', format: '.pkg for Intel', arch: 'x64', installId: 'macos',
      blurb: 'Macs with an Intel chip. Updates itself. Not signed yet.',
      match: [/^IronTeams-osx-x64-Setup\.pkg$/]
    }
  };

  var OS_ORDER = ['linux', 'windows', 'macos'];
  var OS_LABEL = { linux: 'Linux', windows: 'Windows', macos: 'macOS' };

  /* ---- OS detection ---- */
  function detectOS(nav) {
    nav = nav || {};
    var data = nav.userAgentData;
    var platform = String((data && data.platform) || '').toLowerCase();
    var ua = String(nav.userAgent || '');
    var uaLower = ua.toLowerCase();
    var touch = Number(nav.maxTouchPoints || 0) > 1;

    if (platform === 'android' || /android/.test(uaLower)) return 'android';
    if (/iphone|ipad|ipod/.test(uaLower)) return 'ios';
    if (platform === 'windows' || /windows/.test(uaLower)) return 'windows';
    if (platform === 'macos' || /macintosh|mac os x/.test(uaLower)) {
      return touch && !data ? 'ios' : 'macos'; // iPadOS Safari claims to be a Mac but has touch
    }
    if (platform === 'chrome os' || /cros/.test(uaLower)) return 'other';
    if (platform === 'linux' || /linux|x11|ubuntu|fedora/.test(uaLower)) return 'linux';
    return 'other';
  }

  /* Resolves 'arm64', 'x64' or null (unknown). Only Chromium can say; Safari and Firefox
     report an Intel Mac even on Apple Silicon, so null means "show both, recommend neither". */
  function detectMacArch(nav) {
    var data = nav && nav.userAgentData;
    if (!data || typeof data.getHighEntropyValues !== 'function') return Promise.resolve(null);
    return data.getHighEntropyValues(['architecture']).then(function (v) {
      var a = String(v && v.architecture || '').toLowerCase();
      if (a === 'arm') return 'arm64';
      if (a === 'x86') return 'x64';
      return null;
    }, function () { return null; });
  }

  /* ---- releases ---- */
  /* Keeps only what the page needs (also what gets cached) and drops drafts. */
  function normalize(raw) {
    if (!Array.isArray(raw)) return [];
    return raw.filter(function (r) { return r && !r.draft && r.tag_name; }).map(function (r) {
      return {
        tag: String(r.tag_name),
        url: String(r.html_url || RELEASES_PAGE),
        published: r.published_at || r.created_at || null,
        prerelease: !!r.prerelease,
        assets: (Array.isArray(r.assets) ? r.assets : []).map(function (a) {
          return {
            name: String(a.name),
            size: Number(a.size) || 0,
            url: String(a.browser_download_url || ''),
            digest: typeof a.digest === 'string' ? a.digest : '',
            state: a.state || 'uploaded'
          };
        })
      };
    });
  }

  function byNewest(a, b) {
    return String(b.published || '').localeCompare(String(a.published || ''));
  }

  function eligible(releases, beta) {
    return releases.filter(function (r) { return beta || !r.prerelease; }).sort(byNewest);
  }

  function safeUrl(u) {
    return typeof u === 'string' && u.indexOf(DOWNLOAD_PREFIX) === 0;
  }

  function findAsset(release, spec) {
    for (var i = 0; i < spec.match.length; i++) {
      for (var j = 0; j < release.assets.length; j++) {
        var a = release.assets[j];
        if (a.state === 'uploaded' && spec.match[i].test(a.name) && safeUrl(a.url)) return a;
      }
    }
    return null;
  }

  function version(tag) { return String(tag).replace(/^v/, ''); }

  /* For every asset key: the newest eligible release that has the file, or null.
     `latest` is the newest eligible release overall, so the page can say when a platform's
     file lives in an older release (Windows and macOS files are added by a manual run). */
  function pick(releases, beta) {
    var list = eligible(releases, !!beta);
    var out = { latest: list[0] || null, items: {} };
    Object.keys(ASSETS).forEach(function (key) {
      var found = null;
      for (var i = 0; i < list.length && !found; i++) {
        var asset = findAsset(list[i], ASSETS[key]);
        if (asset) {
          found = {
            key: key, spec: ASSETS[key], asset: asset, release: list[i],
            version: version(list[i].tag),
            older: list[i] !== list[0]
          };
        }
      }
      out.items[key] = found;
    });
    return out;
  }

  /* ---- formatting ---- */
  function formatSize(bytes) {
    if (!bytes) return '';
    var mb = bytes / (1024 * 1024);
    return (mb >= 100 ? mb.toFixed(0) : mb.toFixed(1)) + ' MB';
  }

  function formatDate(iso, locale) {
    if (!iso) return '';
    var d = new Date(iso);
    if (isNaN(d.getTime())) return '';
    try {
      return d.toLocaleDateString(locale || undefined, { year: 'numeric', month: 'short', day: 'numeric' });
    } catch (e) {
      return iso.slice(0, 10);
    }
  }

  function shortDigest(digest) {
    var m = /^sha256:([0-9a-f]{64})$/i.exec(digest || '');
    return m ? m[1].toLowerCase() : '';
  }

  /* Which asset key the big hero button points at for a given OS (and Mac arch). */
  function heroKey(os, macArch) {
    if (os === 'windows') return 'windows-setup';
    if (os === 'macos') return macArch === 'x64' ? 'macos-x64' : 'macos-arm64';
    return 'linux-appimage'; // Linux, and the fallback for everything else
  }

  return {
    REPO: REPO, API_URL: API_URL, RELEASES_PAGE: RELEASES_PAGE, DOWNLOAD_PREFIX: DOWNLOAD_PREFIX,
    ASSETS: ASSETS, OS_ORDER: OS_ORDER, OS_LABEL: OS_LABEL,
    detectOS: detectOS, detectMacArch: detectMacArch, normalize: normalize, pick: pick,
    formatSize: formatSize, formatDate: formatDate, shortDigest: shortDigest, heroKey: heroKey
  };
});
