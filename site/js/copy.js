/* Adds a "Copy" button to every <div class="code"><pre>. Without JS the commands stay selectable text. */
(function () {
  'use strict';

  function copyText(text) {
    if (navigator.clipboard && window.isSecureContext) return navigator.clipboard.writeText(text);
    return new Promise(function (resolve, reject) {
      var ta = document.createElement('textarea');
      ta.value = text;
      ta.setAttribute('readonly', '');
      ta.style.position = 'fixed';
      ta.style.opacity = '0';
      document.body.appendChild(ta);
      ta.select();
      var ok = false;
      try { ok = document.execCommand('copy'); } catch (e) { ok = false; }
      document.body.removeChild(ta);
      ok ? resolve() : reject(new Error('copy failed'));
    });
  }

  function init() {
    Array.prototype.forEach.call(document.querySelectorAll('.code'), function (box) {
      var pre = box.querySelector('pre');
      if (!pre || box.querySelector('.copy')) return;
      var btn = document.createElement('button');
      btn.type = 'button';
      btn.className = 'copy';
      btn.textContent = 'Copy';
      btn.setAttribute('aria-label', 'Copy these commands');
      var live = document.createElement('span');
      live.className = 'sr-only';
      live.setAttribute('role', 'status');
      var timer = 0;
      btn.addEventListener('click', function () {
        copyText(pre.innerText.replace(/\s+$/, '')).then(function () {
          btn.textContent = 'Copied';
          btn.classList.add('is-done');
          live.textContent = 'Copied to the clipboard';
        }, function () {
          btn.textContent = 'Press Ctrl+C';
          live.textContent = 'Could not copy. Select the text and press Ctrl+C.';
        });
        clearTimeout(timer);
        timer = setTimeout(function () {
          btn.textContent = 'Copy';
          btn.classList.remove('is-done');
          live.textContent = '';
        }, 2000);
      });
      box.appendChild(btn);
      box.appendChild(live);
    });
  }

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', init);
  else init();
})();
