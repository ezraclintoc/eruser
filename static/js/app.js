/* eruser terminal keyboard.
 *
 * The chrome is keyboard-driven the way the design promises: the numbered
 * tabs move between views, `w` starts a run, and `j`/`k` walk the mail list
 * when one is open. Anything typed into a field is left alone.
 */
(function () {
  'use strict';

  // Numbered tab -> path. Order matches the chrome.
  var TABS = ['/', '/run', '/captchas', '/letters', '/sending'];

  function go(path) {
    if (window.location.pathname !== path) {
      window.location.href = path;
    }
  }

  document.addEventListener('keydown', function (e) {
    var target = e.target || {};
    var tag = (target.tagName || '').toLowerCase();

    // Never steal keys from a field, a modifier combo, or the browser.
    if (tag === 'input' || tag === 'textarea' || tag === 'select' || tag === 'button') {
      if (!(e.metaKey || e.ctrlKey || e.altKey)) {
        // Buttons keep space/enter; everything else in a field is text.
        return;
      }
    }
    if (e.metaKey || e.ctrlKey || e.altKey) return;

    var n = parseInt(e.key, 10);
    if (n >= 1 && n <= TABS.length) {
      e.preventDefault();
      go(TABS[n - 1]);
      return;
    }

    if (e.key === 'w') {
      e.preventDefault();
      go('/run');
      return;
    }

    // j/k walk the mail list, one thread at a time.
    if (e.key === 'j' || e.key === 'k') {
      var rows = Array.prototype.slice.call(document.querySelectorAll('[data-mail-row]'));
      if (!rows.length) return;

      var current = rows.findIndex(function (row) {
        return row.getAttribute('data-mail-row') === 'selected';
      });
      var next = e.key === 'j' ? Math.min(rows.length - 1, current + 1) : Math.max(0, current - 1);
      if (next === current) return;

      e.preventDefault();
      rows.forEach(function (row, index) {
        row.setAttribute('data-mail-row', index === next ? 'selected' : '');
      });
      var link = rows[next].querySelector('a[data-mail-link]');
      if (link) link.focus();
    }
  });

  // The user menu toggles on click and closes when you click elsewhere.
  document.addEventListener('click', function (e) {
    var toggle = e.target.closest('[data-menu-toggle]');
    var panel = document.querySelector('[data-menu]');
    if (!panel) return;

    if (toggle) {
      e.preventDefault();
      panel.hidden = !panel.hidden;
      return;
    }
    if (!panel.hidden && !e.target.closest('[data-menu]')) {
      panel.hidden = true;
    }
  });
})();
