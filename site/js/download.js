/* Download section: reads the releases live from the GitHub REST API (unauthenticated, cached in
   sessionStorage for 15 minutes to stay under the 60 requests per hour limit), maps the files by
   name and renders them. Nothing is hard-coded: versions, sizes and dates all come from the API.
   Works on index.html (hero button + chooser) and install.html ([data-dl-link] buttons). */
(function () {
  'use strict';
  var R = window.IronReleases;
  if (!R) return;

  var CACHE_KEY = 'ironteams-releases-v1';
  var BETA_KEY = 'ironteams-beta';
  var TTL = 15 * 60 * 1000;

  var state = { releases: null, failed: false, beta: false, os: 'linux', detected: 'other', macArch: null };

  /* ---- tiny helpers ---- */
  function $(sel, root) { return (root || document).querySelector(sel); }
  function $all(sel, root) { return Array.prototype.slice.call((root || document).querySelectorAll(sel)); }
  function el(tag, cls, text) {
    var n = document.createElement(tag);
    if (cls) n.className = cls;
    if (text != null) n.textContent = text;
    return n;
  }
  function store(kind) { try { return window[kind]; } catch (e) { return null; } }
  function read(key) { try { var s = store('sessionStorage'); return s ? s.getItem(key) : null; } catch (e) { return null; } }
  function write(key, val) { try { var s = store('sessionStorage'); if (s) s.setItem(key, val); } catch (e) { /* private mode */ } }

  /* ---- loading ---- */
  function readCache() {
    try {
      var c = JSON.parse(read(CACHE_KEY));
      if (c && Array.isArray(c.data)) return c;
    } catch (e) { /* ignore */ }
    return null;
  }

  function fetchReleases() {
    var ctl = typeof AbortController === 'function' ? new AbortController() : null;
    var timer = ctl ? setTimeout(function () { ctl.abort(); }, 10000) : null;
    return fetch(R.API_URL, {
      headers: { Accept: 'application/vnd.github+json' },
      signal: ctl ? ctl.signal : undefined
    }).then(function (res) {
      if (timer) clearTimeout(timer);
      if (!res.ok) throw new Error('GitHub API answered ' + res.status);
      return res.json();
    }).then(function (json) {
      var data = R.normalize(json);
      write(CACHE_KEY, JSON.stringify({ t: Date.now(), data: data }));
      return data;
    });
  }

  function load() {
    var cached = readCache();
    if (cached && Date.now() - cached.t < TTL) return Promise.resolve(cached.data);
    return fetchReleases().catch(function (err) {
      if (cached) return cached.data; // stale beats nothing (rate limit, offline)
      throw err;
    });
  }

  /* ---- rendering ---- */
  function picked() { return R.pick(state.releases || [], state.beta); }

  function metaLine(it) {
    var v = el('strong', null, 'v' + it.version);
    var p = el('span');
    p.appendChild(v);
    var size = R.formatSize(it.asset.size);
    var date = R.formatDate(it.release.published);
    if (size) p.appendChild(document.createTextNode(' · ' + size));
    if (date) p.appendChild(document.createTextNode(' · ' + date));
    return p;
  }

  function dlItem(key, found, opts) {
    var spec = R.ASSETS[key];
    var li = el('li', 'dl-item' + (found ? '' : ' is-missing') + (found && opts.recommended ? ' is-rec' : ''));
    var info = el('div');
    var title = el('div', 'dl-title');
    title.appendChild(el('span', null, spec.format));
    if (found && opts.recommended) title.appendChild(el('span', 'tag', 'Recommended'));
    if (found && found.release.prerelease) title.appendChild(el('span', 'tag tag--beta', 'beta'));
    info.appendChild(title);
    info.appendChild(el('p', 'dl-desc', spec.blurb));

    var actions = el('div', 'dl-actions');
    if (found) {
      var meta = el('p', 'dl-meta');
      meta.appendChild(metaLine(found));
      info.appendChild(meta);
      var latest = picked().latest;
      if (found.older && latest) {
        info.appendChild(el('p', 'dl-old',
          'The newest release, v' + latest.tag.replace(/^v/, '') +
          ', does not have this file yet. This is the newest one that does.'));
      }
      var sha = R.shortDigest(found.asset.digest);
      if (sha) {
        var d = el('details', 'sum');
        d.appendChild(el('summary', null, 'SHA-256 checksum'));
        d.appendChild(el('code', null, sha));
        info.appendChild(d);
      }
      var a = el('a', 'btn', 'Download');
      a.href = found.asset.url;
      a.setAttribute('aria-label', 'Download ' + spec.format + ' ' + found.asset.name + ' version ' + found.version);
      actions.appendChild(a);
    } else {
      info.appendChild(el('p', 'dl-meta', 'Not available yet. ' + notYet(spec.os)));
      var na = el('span', 'btn', 'Not available yet');
      na.setAttribute('aria-disabled', 'true');
      actions.appendChild(na);
    }
    var guide = el('a', 'btn btn--quiet', 'Install guide');
    guide.href = 'install.html#' + spec.installId;
    actions.appendChild(guide);

    li.appendChild(info);
    li.appendChild(actions);
    return li;
  }

  function notYet(os) {
    if (os === 'windows' || os === 'macos') {
      return 'Windows and macOS files are added to a release by a separate build, so no release has them yet.';
    }
    return 'No release has this file.';
  }

  var OS_NOTE = {
    linux: 'Linux is the system IronTeams is used on every day (KDE Plasma, Wayland). If you are not sure which file to pick, take the AppImage.',
    windows: 'The Windows build is tested in CI but has not been tried on real Windows hardware yet. The installer is not signed: SmartScreen asks once.',
    macos: 'The macOS build is tested in CI but has not been tried on real Mac hardware yet. The package is not signed: open it with right click, then Open.'
  };

  function keysFor(os) {
    var keys = Object.keys(R.ASSETS).filter(function (k) { return R.ASSETS[k].os === os; });
    if (os === 'macos' && state.macArch === 'x64') keys.reverse();
    return keys;
  }

  function recommended(key) {
    var spec = R.ASSETS[key];
    if (spec.os === 'macos') return state.macArch ? spec.arch === state.macArch : false;
    return !!spec.recommended;
  }

  function renderChooser() {
    var list = $('#dl-list');
    if (!list) return;
    var status = $('#dl-status');
    var note = $('#dl-os-note');
    $all('#dl-os button').forEach(function (b) {
      b.setAttribute('aria-pressed', String(b.getAttribute('data-os') === state.os));
    });
    if (note) note.textContent = OS_NOTE[state.os] || '';
    list.textContent = '';

    if (state.failed && !state.releases) {
      status.className = 'dl-status is-error';
      status.textContent = 'Could not read the release list from GitHub (offline, or its rate limit is used up). Use the releases page instead.';
      var li = el('li', 'dl-item');
      var a = el('a', 'btn', 'Open the releases page');
      a.href = R.RELEASES_PAGE;
      li.appendChild(a);
      list.appendChild(li);
      return;
    }
    if (!state.releases) return;

    var p = picked();
    status.className = 'dl-status';
    if (!p.latest) {
      status.textContent = state.beta ? 'No releases are published yet.' : 'No stable release is published yet. Turn on beta releases to see pre-releases.';
    } else {
      status.textContent = 'Latest ' + (p.latest.prerelease ? 'beta' : 'release') + ': v' + p.latest.tag.replace(/^v/, '') +
        ', published ' + R.formatDate(p.latest.published) + '. Files are read from GitHub just now.';
    }
    keysFor(state.os).forEach(function (key) {
      list.appendChild(dlItem(key, p.items[key], { recommended: recommended(key) }));
    });
  }

  function osName(os) { return R.OS_LABEL[os] || ''; }

  function renderHero() {
    var a = $('#hero-download');
    if (!a) return;
    var label = $('.hero-label', a);
    var sub = $('.hero-sub', a);
    var others = $('#hero-others');
    var os = state.detected;
    var desktop = os === 'linux' || os === 'windows' || os === 'macos';

    function set(href, l, s) { a.href = href; label.textContent = l; sub.textContent = s; }

    if (state.failed && !state.releases) {
      set(R.RELEASES_PAGE, 'Download' + (desktop ? ' for ' + osName(os) : ''), 'Open the GitHub releases page');
    } else if (!state.releases) {
      set('#download', 'Download' + (desktop ? ' for ' + osName(os) : ''), 'Checking the latest release…');
    } else if (os === 'android' || os === 'ios') {
      set('#download', 'IronTeams is a desktop app', 'See the downloads for Linux, Windows and macOS');
    } else if (!desktop) {
      set('#download', 'Choose your download', 'Linux, Windows and macOS builds');
    } else {
      var key = R.heroKey(os, state.macArch);
      var found = picked().items[key];
      if (found) {
        var bits = [found.spec.format, 'v' + found.version];
        var size = R.formatSize(found.asset.size);
        if (size) bits.push(size);
        set(found.asset.url, 'Download for ' + osName(os), bits.join(' · '));
        if (os === 'macos' && !state.macArch) sub.textContent += ' · pick Intel below if your Mac is older than 2020';
      } else {
        set('#download', 'Download for ' + osName(os), 'Not available yet – see all downloads');
      }
    }

    if (others) {
      others.textContent = '';
      if (desktop) others.appendChild(document.createTextNode('Detected ' + osName(os) + '. Other system: '));
      else others.appendChild(document.createTextNode('Pick your system: '));
      R.OS_ORDER.filter(function (o) { return o !== os; }).forEach(function (o, i, arr) {
        var l = el('a', null, osName(o));
        l.href = '#download';
        l.setAttribute('data-os', o);
        others.appendChild(l);
        if (i < arr.length - 1) others.appendChild(document.createTextNode(', '));
      });
      others.appendChild(document.createTextNode('.'));
    }
  }

  function renderLinks() {
    if (!state.releases && !state.failed) return;
    var p = state.releases ? picked() : null;
    $all('[data-dl-link]').forEach(function (a) {
      var key = a.getAttribute('data-dl-link');
      var spec = R.ASSETS[key];
      if (!spec) return;
      var found = p && p.items[key];
      var meta = a.parentNode && $('[data-dl-meta="' + key + '"]', a.parentNode);
      if (found) {
        a.href = found.asset.url;
        a.removeAttribute('aria-disabled');
        a.textContent = 'Download ' + spec.format;
        if (meta) {
          meta.textContent = 'v' + found.version + ' · ' + R.formatSize(found.asset.size) + ' · ' + R.formatDate(found.release.published) +
            (found.older ? ' · not in the newest release yet' : '');
        }
      } else if (p) {
        a.href = R.RELEASES_PAGE;
        a.setAttribute('aria-disabled', 'true');
        a.textContent = 'Not available yet';
        if (meta) meta.textContent = 'No release has this file yet.';
      } else if (meta) {
        meta.textContent = 'Could not read the release list. The releases page has every file.';
      }
    });
  }

  function renderAll() { renderHero(); renderChooser(); renderLinks(); }

  /* ---- wiring ---- */
  function selectOS(os) {
    if (OS_OK(os)) { state.os = os; renderChooser(); }
  }
  function OS_OK(os) { return R.OS_ORDER.indexOf(os) !== -1; }

  function init() {
    var nav = window.navigator;
    state.detected = R.detectOS(nav);
    state.os = OS_OK(state.detected) ? state.detected : 'linux';
    state.beta = read(BETA_KEY) === '1';

    var beta = $('#dl-beta');
    if (beta) {
      beta.checked = state.beta;
      beta.addEventListener('change', function () {
        state.beta = beta.checked;
        write(BETA_KEY, state.beta ? '1' : '0');
        renderAll();
      });
    }
    $all('#dl-os button').forEach(function (b) {
      b.addEventListener('click', function () { selectOS(b.getAttribute('data-os')); });
    });
    document.addEventListener('click', function (e) {
      var t = e.target.closest ? e.target.closest('a[data-os]') : null;
      if (t) selectOS(t.getAttribute('data-os')); // then the browser jumps to #download
    });

    renderAll();
    if (state.detected === 'macos') {
      R.detectMacArch(nav).then(function (arch) {
        state.macArch = arch;
        renderAll();
      });
    }
    load().then(function (data) {
      state.releases = data;
      renderAll();
    }, function () {
      state.failed = true;
      renderAll();
    });
  }

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', init);
  else init();
})();
