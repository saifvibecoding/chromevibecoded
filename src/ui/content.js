// Injected into the main frame of every web page Rustium loads.
// Provides: keyboard shortcuts, SPA URL tracking, media detection (so playing tabs never
// sleep) and the password manager's autofill / save-on-submit.
(() => {
  if (window.__rustium) return;
  Object.defineProperty(window, '__rustium', { value: true });
  const post = (m) => { try { window.ipc.postMessage(JSON.stringify(m)); } catch (_) {} };

  // ---- keyboard shortcuts (Chrome-compatible) -------------------------------------------
  addEventListener('keydown', (e) => {
    const c = e.ctrlKey || e.metaKey, k = e.key, l = k.length === 1 ? k.toLowerCase() : k;
    let a = null;
    if (c && e.shiftKey && l === 't') a = 'reopen';
    else if (c && e.shiftKey && l === 'g') a = 'gamemode';
    else if (c && e.shiftKey && l === 'o') a = 'bookmarks';
    else if (c && !e.shiftKey && l === 't') a = 'newtab';
    else if (c && (l === 'w' || k === 'F4')) a = 'close';
    else if ((c && l === 'l') || (e.altKey && l === 'd') || k === 'F6') a = 'focus';
    else if (c && k === 'Tab') a = e.shiftKey ? 'prev' : 'next';
    else if (c && k === 'PageDown') a = 'next';
    else if (c && k === 'PageUp') a = 'prev';
    else if (c && /^[1-9]$/.test(k)) a = 'tab' + k;
    else if (c && l === 'd') a = 'bookmark';
    else if (c && l === 'h') a = 'history';
    else if (c && l === 'j') a = 'downloads';
    else if (e.altKey && k === 'ArrowLeft') a = 'back';
    else if (e.altKey && k === 'ArrowRight') a = 'forward';
    else if (e.altKey && k === 'Home') a = 'home';
    else if (k === 'F5' || (c && l === 'r')) a = 'reload';
    else if (k === 'F12' || (c && e.shiftKey && l === 'i')) a = 'devtools';
    if (a) { e.preventDefault(); e.stopImmediatePropagation(); post({ t: 'key', k: a }); }
  }, true);

  // Mouse back/forward buttons
  addEventListener('mouseup', (e) => {
    if (e.button === 3) post({ t: 'key', k: 'back' });
    if (e.button === 4) post({ t: 'key', k: 'forward' });
  }, true);

  // ---- URL changes in single-page apps ---------------------------------------------------
  let urlTimer = 0, lastUrl = location.href;
  const urlChanged = () => {
    clearTimeout(urlTimer);
    urlTimer = setTimeout(() => {
      if (location.href !== lastUrl) { lastUrl = location.href; post({ t: 'url' }); queried = false; scan(); }
    }, 250);
  };
  for (const f of ['pushState', 'replaceState']) {
    const orig = history[f];
    history[f] = function () { const r = orig.apply(this, arguments); urlChanged(); return r; };
  }
  addEventListener('popstate', urlChanged);
  addEventListener('hashchange', urlChanged);

  // ---- media: tell the browser when audio/video plays ------------------------------------
  let playing = false;
  const mediaCheck = () => {
    const now = [...document.querySelectorAll('video,audio')].some((m) => !m.paused && !m.ended && !m.muted && m.volume > 0);
    if (now !== playing) { playing = now; post({ t: 'media', on: now }); }
  };
  for (const ev of ['play', 'playing', 'pause', 'ended', 'volumechange', 'emptied']) addEventListener(ev, mediaCheck, true);

  // ---- passwords ---------------------------------------------------------------------------
  const visible = (el) => el && !el.disabled && !el.readOnly && el.getClientRects().length > 0;
  const isUserField = (el) => {
    const t = (el.getAttribute('type') || 'text').toLowerCase();
    const ac = (el.autocomplete || '').toLowerCase();
    return ac.includes('username') || ac === 'email' || ['text', 'email', 'tel'].includes(t);
  };
  const findFields = () => {
    const pw = [...document.querySelectorAll('input[type=password]')].find(visible) || null;
    const scope = (pw && pw.form) || document;
    const inputs = [...scope.querySelectorAll('input')].filter(visible);
    let user = null;
    if (pw) {
      for (let i = inputs.indexOf(pw) - 1; i >= 0; i--) if (isUserField(inputs[i])) { user = inputs[i]; break; }
    }
    if (!user) user = [...document.querySelectorAll('input[autocomplete~=username],input[type=email],input[name*=user i],input[name*=email i],input[id*=user i],input[id*=email i]')].find(visible) || null;
    return { pw, user };
  };
  const setVal = (el, v) => {
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    el.focus();
    setter.call(el, v);
    el.dispatchEvent(new Event('input', { bubbles: true }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
    el.style.backgroundColor = 'rgba(138,180,248,.18)';
  };

  // Remember a typed username across two-step logins (e.g. Google: email page, then password page).
  const remember = (v) => { try { sessionStorage.setItem('__rs_user', v); } catch (_) {} };
  const recalled = () => { try { return sessionStorage.getItem('__rs_user') || ''; } catch (_) { return ''; } };
  addEventListener('change', (e) => {
    const el = e.target;
    if (el instanceof HTMLInputElement && el.type !== 'password' && isUserField(el) && el.value) remember(el.value);
  }, true);

  // Called by the browser (after it has verified the origin) with saved logins for this site.
  let creds = [];
  Object.defineProperty(window, '__rsFill', {
    value: (list) => { creds = list || []; fill(); },
  });
  const fill = () => {
    if (!creds.length) return;
    const { pw, user } = findFields();
    const typed = user && user.value;
    const pick = creds.find((c) => c.u && (c.u === typed || c.u === recalled())) || creds[0];
    if (user && !user.value && pick.u) setVal(user, pick.u);
    if (pw && !pw.value && (!typed || typed === pick.u)) setVal(pw, pick.p);
  };

  let queried = false;
  const scan = () => {
    const { pw, user } = findFields();
    if (!pw && !user) return;
    if (!queried) { queried = true; post({ t: 'pw_query' }); } else fill();
  };

  let lastOffer = '';
  const capture = () => {
    const { pw, user } = findFields();
    if (!pw || !pw.value) return;
    const u = (user && user.value) || recalled();
    const key = u + '\u0000' + pw.value;
    if (key === lastOffer) return;
    lastOffer = key;
    post({ t: 'pw_save', u, p: pw.value });
  };
  addEventListener('submit', capture, true);
  addEventListener('keydown', (e) => { if (e.key === 'Enter' && e.target && e.target.type === 'password') capture(); }, true);
  addEventListener('click', (e) => {
    const b = e.target && e.target.closest && e.target.closest('button,input[type=submit],[role=button]');
    if (b && findFields().pw) setTimeout(capture, 0);
  }, true);

  // Watch for login forms rendered late by JS, throttled, and only for the first 30s of a
  // page so idle tabs cost zero CPU (SPA navigations re-scan via urlChanged).
  let scanTimer = 0;
  const throttledScan = () => { if (!scanTimer) scanTimer = setTimeout(() => { scanTimer = 0; scan(); }, 500); };
  const start = () => {
    scan();
    const mo = new MutationObserver(throttledScan);
    mo.observe(document.documentElement, { childList: true, subtree: true });
    setTimeout(() => mo.disconnect(), 30000);
  };
  if (document.readyState === 'loading') addEventListener('DOMContentLoaded', start, { once: true }); else start();
})();
