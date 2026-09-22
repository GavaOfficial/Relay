'use strict';

(function () {
  const listeners = {};
  const clone = (x) => JSON.parse(JSON.stringify(x));
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const emit = (n, p) => (listeners[n] || []).forEach((f) => f({ payload: clone(p) }));
  const isOverlay = /overlay\.html$/.test(location.pathname);
  let bc = null;
  try { bc = new BroadcastChannel('relay-mock'); } catch (_) {}

  window.__TAURI__ = {
    event: { listen: async (n, f) => { (listeners[n] = listeners[n] || []).push(f); return () => {}; } },
    core: { invoke: (cmd, args) => (isOverlay ? overlayInvoke(cmd) : handle(cmd, args || {})) },
  };

  function overlayInvoke(cmd) {
    if (cmd !== 'get_state') return Promise.reject('non supportato');
    const d = new URLSearchParams(location.search).get('demo');
    if (d) return Promise.resolve(demoSnap(d));
    try { return Promise.resolve(JSON.parse(localStorage.getItem('relay-mock-state'))); } catch (_) { return Promise.resolve(null); }
  }
  function demoSnap(phase) {
    return { auth: { logged_in: true, name: 'Ada', user: 'u-ada' }, connection: 'connected', error: null,
      match: { id: 'x', role: 'player', phase, invite_url: null, replay_url: 'https://relay.example.com/matches/x',
        started_at_ms: Date.now() - 754000, stopped_at_ms: null, clock_offset_ms: 0, players: [], can_start: false, start_blockers: [], me: null } };
  }
  if (isOverlay) {
    if (bc && !new URLSearchParams(location.search).get('demo')) bc.onmessage = (e) => emit('state', e.data);
    return;
  }

  const ME = { id: 'u-ada', name: 'Ada' };
  const st = {
    auth: { logged_in: false, name: null, user: null }, connection: 'connected', error: null, match: null,
    capture: { state: 'ready', stage: 'check', percent: 1, groups: [], error: null, compat: { ok: true, best_encoder: 'nvenc' } },
  };
  if (new URLSearchParams(location.search).get('install')) {
    st.capture = { state: 'installing', stage: 'obs', percent: 0, groups: [
      { id: 'engine', label: 'Motore di cattura', percent: 0, done: false },
      { id: 'encoders', label: 'Codifica video e audio', percent: 0, done: false },
      { id: 'games', label: 'Aggancio ai giochi', percent: 0, done: false },
    ], error: null, compat: null };
    let step = 0;
    const tick = setInterval(() => {
      step += 5;
      st.capture.percent = Math.min(100, step) / 100;
      st.capture.groups.forEach((g, i) => { g.percent = Math.max(0, Math.min(100, step - i * 20)); g.done = g.percent >= 100; });
      if (step >= 90 && st.capture.stage === 'obs') st.capture.stage = 'exe';
      if (step >= 130) {
        st.capture.stage = 'check';
        st.capture.percent = 1;
      }
      if (step >= 160) {
        clearInterval(tick);
        st.capture = { state: 'ready', stage: 'check', percent: 1, groups: [], error: null, compat: { ok: true, best_encoder: 'nvenc' } };
      }
      push();
    }, 400);
  }
  let settings = {
    server_url: 'https://relay.gavatech.org', auth_url: 'https://auth.gavatech.org', site_url: '',
    window: null, preset: 'high', fps: 60, bitrate_kbps: 6000, limit_kbps: null,
    encoder: 'auto', ffmpeg_path: 'ffmpeg', last_speedtest: null, overlay: true, audio_game: true, audio_mic: false, audio_mic_gain: 3,
  };
  const past = [
    { id: 'm-old1', players: ['Ada', 'Bob'], status: 'done', created_at: Math.floor(Date.now() / 1000) - 86400, host: true },
    { id: 'm-old2', players: ['Zoe', 'Ada', 'Cleo'], status: 'waiting', created_at: Math.floor(Date.now() / 1000) - 3600, host: false },
  ];
  const WINDOWS = [
    { title: 'Schermo intero: monitor 1 (2560\u00d71440)', exe: '@monitor:0' },
    { title: 'Schermo intero: monitor 2 (1920\u00d71080)', exe: '@monitor:1' },
    { title: 'Mio Gioco', exe: 'game.exe' }, { title: 'Mio Gioco - Editor', exe: 'game.exe' },
    { title: 'Counter-Strike 2', exe: 'cs2.exe' }, { title: 'Chrome - Una pagina con un titolo molto molto lungo che va a capo', exe: 'chrome.exe' },
    { title: 'Blocco note', exe: 'notepad.exe' }, { title: 'Discord', exe: 'discord.exe' },
  ];
  let slowUpload = false, sim = null, script = [], tickT = null;

  function mkPlayer(id, name, extra) {
    return Object.assign({ id, name, connected: false, ready: false, state: 'idle', window_found: null,
      backlog: 0, upload_kbps: 0, rtt_ms: null, offset_ms: null, issue: null, host: false,
      _forceErr: false, _rtt: 24, _backlog: null }, extra || {});
  }

  function recompute() {
    const m = st.match;
    if (!m) return;
    for (const p of m.players) {
      if (!p.connected) { p.ready = false; p.state = 'idle'; p.issue = null; p.rtt_ms = null; p.upload_kbps = 0; continue; }
      p.rtt_ms = p._rtt + (m.phase === 'recording' ? Math.round(Math.random() * 4) : 0);
      p.offset_ms = 3;
      p.issue = null;
      if (p.id === ME.id) {
        p.window_found = settings.window ? settings.window.exe.startsWith('@monitor:') || WINDOWS.some((w) => w.exe === settings.window.exe) : false;
        p.window = settings.window ? settings.window.exe + ' - ' + settings.window.title : null;
      }
      if (p._forceErr) { p.state = 'error'; p.issue = 'encoder non avviato'; p.ready = false; continue; }
      if (p.window_found === false) p.issue = p.id === ME.id && !settings.window ? 'finestra non scelta' : 'finestra non trovata';
      else if (p.rtt_ms > 200) p.issue = 'ping alto';
      else if ((p._backlog ?? 0) > 5) p.issue = 'coda upload alta';
      p.ready = !p.issue;
      if (m.phase === 'lobby') p.state = p.ready ? 'idle' : 'waiting';
      else if (m.phase === 'recording') {
        p.state = 'recording';
        p.backlog = p._backlog ?? Math.floor(Math.random() * 3);
        p.upload_kbps = slowUpload ? 900 : 4800 + Math.round(Math.random() * 900);
      } else if (m.phase === 'uploading' || m.phase === 'stopping') {
        p.state = p.backlog > 0 ? 'uploading' : 'idle';
        p.upload_kbps = p.backlog > 0 ? 5200 : 0;
      } else { p.state = 'idle'; p.backlog = 0; p.upload_kbps = 0; }
    }
    m.can_start = m.phase === 'lobby' && m.players.length > 0 && m.players.every((p) => p.ready);
    m.start_blockers = m.phase !== 'lobby' ? [] : m.players.length === 0 ? ['Nessun giocatore connesso']
      : m.players.filter((p) => !p.ready).map((p) => p.name + ': ' + (p.issue || (p.connected ? 'non pronto' : 'non connesso')));
    const me = m.players.find((p) => p.id === ME.id);
    m.me = me ? { window_found: me.window_found, encoder: 'h264_nvenc', backlog: me.backlog, upload_kbps: me.upload_kbps, rtt_ms: me.rtt_ms, offset_ms: me.offset_ms } : null;
  }
  function snapshot() {
    recompute();
    const s = clone(st);
    if (s.match) s.match.players.forEach((p) => {
      for (const k of Object.keys(p)) if (k[0] === '_') delete p[k];

      if (s.match.role === 'player') delete p.window;
    });
    return s;
  }
  function push() {
    const s = snapshot();
    emit('state', s);
    try { localStorage.setItem('relay-mock-state', JSON.stringify(s)); } catch (_) {}
    if (bc) bc.postMessage(s);
  }
  const later = (ms, fn) => { const t = setTimeout(() => { fn(); push(); }, ms); script.push(t); };
  const pl = (id) => st.match && st.match.players.find((p) => p.id === id);
  function clearScript() { script.forEach(clearTimeout); script = []; clearInterval(tickT); tickT = null; }

  function newMatch(role, playing, id, hostName) {
    clearScript();
    id = id || 'm-' + Math.random().toString(16).slice(2, 10);
    st.match = { id, role, phase: 'lobby', invite_url: role === 'player' ? null : 'https://relay.example.com/join/' + id + '?code=K7Q2XA',
      replay_url: 'https://relay.example.com/matches/' + id, started_at_ms: null, stopped_at_ms: null,
      clock_offset_ms: 0, players: [], can_start: false, start_blockers: [], me: null };
    const m = st.match;
    if (role === 'player') m.players.push(mkPlayer('u-zoe', hostName || 'Zoe', { connected: true, window_found: true, host: true }));
    if (playing) {
      const me = mkPlayer(ME.id, ME.name, { host: role === 'host_player' });
      m.players.push(me);
      later(300, () => { me.connected = true; });
      later(1000, () => { me.window_found = true; });
    }
    later(1500, () => { m.players.push(mkPlayer('u-bob', 'Bob', { connected: true, window_found: false, window: 'cs2.exe - Counter-Strike 2' })); });
    later(3000, () => { m.players.push(mkPlayer('u-cleo', 'Cleo', { connected: true, window: 'game.exe - Mio Gioco' })); });
    later(4500, () => { const c = pl('u-cleo'); if (c) c.window_found = true; });
    later(9000, () => { const b = pl('u-bob'); if (b) b.window_found = true; });
    tickT = setInterval(push, 500);
    push();
  }
  function startRec() {
    const m = st.match;
    m.phase = 'recording'; m.started_at_ms = Date.now();
    push();
  }
  function stopRec() {
    const m = st.match;
    m.phase = 'stopping'; m.stopped_at_ms = Date.now();
    m.players.forEach((p) => { if (p.connected) { p.backlog = 6; p._backlog = null; } });
    push();
    later(1000, () => { m.phase = 'uploading'; });
    for (let i = 1; i <= 6; i++) later(1000 + i * 700, () => { m.players.forEach((p) => { if (p.backlog > 0) p.backlog--; }); if (i === 6) m.phase = 'done'; });
  }
  function leave() {
    if (st.match) past.unshift({ id: st.match.id, players: st.match.players.map((p) => p.name), status: st.match.phase === 'done' ? 'done' : 'waiting', created_at: Math.floor(Date.now() / 1000), host: st.match.role !== 'player' });
    clearScript(); st.match = null; push();
  }

  async function handle(cmd, a) {
    await sleep(80);
    switch (cmd) {
      case 'get_state': return snapshot();
      case 'login': await sleep(1400); st.auth = { logged_in: true, name: ME.name, user: ME.id }; push(); return;
      case 'logout': clearScript(); st.auth = { logged_in: false, name: null, user: null }; st.match = null; push(); return;
      case 'get_settings': return clone(settings);
      case 'set_window': settings.window = a.window || null; return;
      case 'check_update': return { state: 'uptodate', version: '0.2.1' };
      case 'open_logs': return;
      case 'rename_match': if (st.match) st.match.name = (a.name || '').trim() || null; return;
      case 'set_audio': settings.audio_game = !!a.game; settings.audio_mic = !!a.mic; settings.audio_mic_gain = a.micGain || 3; return;
      case 'save_settings': settings = clone(a.settings); return clone(settings);
      case 'list_windows': await sleep(300); return clone(WINDOWS).sort(() => 0);
      case 'run_speedtest': {
        const mbps = slowUpload ? 6.3 : 22.4;
        for (let i = 1; i <= 10; i++) { await sleep(250); emit('speedtest', { progress: i / 10, mbps: mbps * (0.7 + 0.03 * i) }); }
        const lim = Math.round(mbps * 500);
        const presets = [['low', 'Leggera', 30, 2500], ['medium', 'Standard', 60, 4000], ['high', 'Alta', 60, 6000], ['max', 'Massima', 60, 9000]]
          .map(([id, label, fps, b]) => ({ id, label, fps, bitrate_kbps: b, fits: b <= lim }));
        const rec = presets.filter((p) => p.fits).pop() || presets[0];
        return { upload_mbps: mbps, recommended: rec.id, limit_kbps: lim, presets };
      }
      case 'create_match': newMatch('host_player', true); return { id: st.match.id, invite_url: st.match.invite_url };
      case 'join_match': {
        const mt = /\/join\/([^/?#]+)/.exec(a.link || '');
        if (!mt || !/code=/.test(a.link)) throw 'Link di invito non valido.';
        if (/fail/.test(a.link)) throw 'Partita non trovata o gia’ conclusa.';
        newMatch('player', true, mt[1]);
        later(6000, () => { if (st.match && st.match.role === 'player' && st.match.phase === 'lobby') startRec(); });
        later(16000, () => { if (st.match && st.match.role === 'player' && st.match.phase === 'recording') stopRec(); });
        return { id: mt[1] };
      }
      case 'open_match': { const r = past.find((x) => x.id === a.id); newMatch(r && r.host ? 'host_player' : 'player', true, a.id); return; }
      case 'leave_match': leave(); return;
      case 'list_matches': return clone(past).slice(0, 10);
      case 'host_start':
        if (!st.match || st.match.role === 'player') throw 'Solo l’host puo’ avviare.';
        recompute();
        if (!st.match.can_start && !a.force) throw 'Non tutti i giocatori sono pronti.';
        startRec(); return;
      case 'host_stop': stopRec(); return;
      case 'open_url': log('open_url ' + a.url); return;
      default: throw 'Comando sconosciuto: ' + cmd;
    }
  }

  let logEl;
  function log(t) { if (logEl) logEl.textContent = t; console.log('[mock]', t); }
  const DEBUG = {
    'Bob offline/online': () => { const p = pl('u-bob'); if (p) p.connected = !p.connected; },
    'Bob finestra ok/no': () => { const p = pl('u-bob'); if (p) p.window_found = !p.window_found; },
    'Coda alta (Bob)': () => { const p = pl('u-bob'); if (p) { p._backlog = p._backlog ? null : 14; p.backlog = p._backlog ?? 0; } },
    'Ping alto (Cleo)': () => { const p = pl('u-cleo'); if (p) p._rtt = p._rtt > 100 ? 24 : 380; },
    'Errore giocatore (Cleo)': () => { const p = pl('u-cleo'); if (p) p._forceErr = !p._forceErr; },
    'Errore globale': () => { st.error = st.error ? null : 'Impossibile inviare i pezzi: server non raggiungibile.'; },
    'Riconnessione': () => { st.connection = st.connection === 'reconnecting' ? 'connected' : 'reconnecting'; },
    'Offline': () => { st.connection = st.connection === 'offline' ? 'connected' : 'offline'; },
    'Upload lento': () => { slowUpload = !slowUpload; log('upload lento: ' + slowUpload); },
    'Fase errore': () => { if (st.match) { st.match.phase = 'error'; st.error = 'Registrazione interrotta.'; } },
  };
  window.__mock = { act: (n) => { DEBUG[n](); push(); }, state: () => st };

  function mountPanel() {
    const box = document.createElement('div');
    box.style.cssText = 'position:fixed;left:4px;bottom:4px;z-index:99;font:11px system-ui;';
    const panel = document.createElement('div');
    panel.hidden = true;
    panel.style.cssText = 'background:#000d;color:#fff;padding:6px;border-radius:6px;max-width:300px;display:flex;flex-wrap:wrap;gap:3px;margin-bottom:3px;';
    for (const n of Object.keys(DEBUG)) {
      const b = document.createElement('button');
      b.textContent = n; b.style.cssText = 'font:11px system-ui;padding:2px 5px;min-height:0;';
      b.onclick = () => window.__mock.act(n);
      panel.append(b);
    }
    logEl = document.createElement('div'); logEl.style.cssText = 'width:100%;opacity:.8;';
    panel.append(logEl);
    const tg = document.createElement('button');
    tg.textContent = 'dbg'; tg.style.cssText = 'font:11px system-ui;padding:1px 6px;min-height:0;opacity:.7;';
    tg.onclick = () => { panel.hidden = !panel.hidden; };
    box.append(panel, tg);
    document.body.append(box);
    document.body.style.paddingBottom = '28px';
  }
  if (document.body) mountPanel(); else document.addEventListener('DOMContentLoaded', mountPanel);
  push();
})();
