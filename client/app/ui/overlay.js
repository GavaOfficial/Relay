'use strict';

(async function () {
  if (!window.__TAURI__ && /[?&]mock=1|#mock/.test(location.search + location.hash)) {
    await new Promise((res) => {
      const s = document.createElement('script');
      s.src = 'mock.js'; s.onload = res; s.onerror = res;
      document.head.append(s);
    });
  }
  const T = window.__TAURI__;
  if (!T) return;
  const pill = document.getElementById('pill'), t = document.getElementById('t');
  let m = null, timer = null;

  function fmt(ms) {
    const s = Math.max(0, Math.floor(ms / 1000));
    const hh = Math.floor(s / 3600), mm = Math.floor((s % 3600) / 60), ss = String(s % 60).padStart(2, '0');
    return hh ? hh + ':' + String(mm).padStart(2, '0') + ':' + ss : mm + ':' + ss;
  }
  function tick() {
    if (m && m.started_at_ms != null) t.textContent = fmt(Date.now() + (m.clock_offset_ms ?? 0) - m.started_at_ms);
  }
  function render(s) {
    const x = s && s.match;
    m = x && x.phase === 'recording' && (x.role === 'player' || x.role === 'host_player') ? x : null;
    pill.hidden = !m;
    if (m) { tick(); if (!timer) timer = setInterval(tick, 250); }
    else if (timer) { clearInterval(timer); timer = null; }
  }
  T.event.listen('state', (e) => render(e.payload));
  try { render(await T.core.invoke('get_state')); } catch (_) {}
})();
