'use strict';

const PRESETS = [
  { id: 'low', label: 'Leggera', fps: 30, bitrate_kbps: 2500 },
  { id: 'medium', label: 'Standard', fps: 60, bitrate_kbps: 4000 },
  { id: 'high', label: 'Alta', fps: 60, bitrate_kbps: 6000 },
  { id: 'max', label: 'Massima', fps: 60, bitrate_kbps: 9000 },
];
const ENCODERS = [
  ['auto', 'Automatico'], ['nvenc', 'NVIDIA (nvenc)'], ['amf', 'AMD (amf)'],
  ['qsv', 'Intel (qsv)'], ['x264', 'Processore (software)'],
];
const STATUS_MATCH = { waiting: 'In attesa', recording: 'In registrazione', done: 'Completata' };

let T = null;
let snap = null;

const ui = {
  view: 'main', prevView: null, busy: null, notice: null, dismissedErr: null,
  loginErr: null, joinErr: null, joinLink: '', newName: '', renaming: false, renameDraft: '', showJoin: false,
  recents: null, recentsLoading: false, thumbs: {},
  copied: false, forceArmed: false, open: new Set(),
  set: null, setDirty: false, saveMsg: '', saveTimer: null,
  windows: null, winBusy: false, winErr: null,
  steam: null, steamChecked: false, game: null, gameQuery: '', gameResults: [], gameBusy: false,
  speed: { running: false, progress: 0, mbps: 0, result: null, error: null },
  advOpen: false, rev: 0,
};

const $ = (id) => document.getElementById(id);
const errMsg = (e) => (typeof e === 'string' ? e : (e && e.message) || String(e));
const invoke = (cmd, args) => T.core.invoke(cmd, args);

function h(tag, attrs, ...kids) {
  const e = document.createElement(tag);
  for (const k in attrs || {}) {
    const v = attrs[k];
    if (v == null || v === false) continue;
    if (k === 'class') e.className = v;
    else if (k === 'text') e.textContent = v;
    else if (k === 'value' || k === 'checked' || k === 'disabled' || k === 'readOnly') e[k] = v;
    else e.setAttribute(k, v === true ? '' : v);
  }
  for (const c of kids.flat()) if (c != null && c !== false) e.append(c);
  return e;
}

const SVGNS = 'http://www.w3.org/2000/svg';
const ICONS = {
  edit: ['M12 20h9', 'M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z'],
  link: ['M10 13a5 5 0 0 0 7.5.5l3-3a5 5 0 0 0-7-7l-1.7 1.7', 'M14 11a5 5 0 0 0-7.5-.5l-3 3a5 5 0 0 0 7 7l1.7-1.7'],
  check: ['M20 6 9 17l-5-5'],
  arrow: ['M5 12h14', 'M13 6l6 6-6 6'],
  refresh: ['M21 12a9 9 0 1 1-3-6.7', 'M21 4v5h-5'],
  chevron: ['M9 6l6 6-6 6'],
  close: ['M6 6l12 12', 'M18 6 6 18'],
  back: ['M15 18l-6-6 6-6'],
  play: ['M8 5.5v13l10.5-6.5z'],
  external: ['M14 4h6v6', 'M20 4l-9 9', 'M19 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1h5'],
  monitor: ['M3 5h18v11H3z', 'M8 20h8', 'M12 16v4'],
  gamepad: ['M6 9h12a4 4 0 0 1 4 4v1a3 3 0 0 1-5.4 1.8L15 14H9l-1.6 1.8A3 3 0 0 1 2 14v-1a4 4 0 0 1 4-4z', 'M7 11.5v3', 'M5.5 13h3'],
  mic: ['M12 3a3 3 0 0 0-3 3v6a3 3 0 0 0 6 0V6a3 3 0 0 0-3-3z', 'M5 11a7 7 0 0 0 14 0', 'M12 18v3'],
  speaker: ['M4 9h4l5-4v14l-5-4H4z', 'M17 9a4 4 0 0 1 0 6'],
  copy: ['M9 9h11v11H9z', 'M5 15H4V4h11v1'],
};
function ico(name, size = 16) {
  const s = document.createElementNS(SVGNS, 'svg');
  for (const [k, v] of Object.entries({ viewBox: '0 0 24 24', width: size, height: size, fill: 'none', stroke: 'currentColor',
    'stroke-width': 2, 'stroke-linecap': 'round', 'stroke-linejoin': 'round', 'aria-hidden': 'true' })) s.setAttribute(k, v);
  for (const d of ICONS[name]) { const p = document.createElementNS(SVGNS, 'path'); p.setAttribute('d', d); s.append(p); }
  return s;
}

function region(id, sig, build) {
  const r = $(id);
  if (r.dataset.sig === sig) return;
  r.dataset.sig = sig;
  const a = document.activeElement;
  const key = a && r.contains(a) ? a.dataset.k : null;
  let s0, s1;
  try { s0 = a.selectionStart; s1 = a.selectionEnd; } catch (_) {}
  r.replaceChildren(...build().flat().filter((x) => x != null && x !== false));
  if (key) {
    const n = r.querySelector('[data-k="' + key + '"]');
    if (n) {
      n.focus();
      try { if (s0 != null) n.setSelectionRange(s0, s1); } catch (_) {}
    }
  }
}

const fmtTime = (ms) => {
  const s = Math.max(0, Math.floor(ms / 1000));
  return Math.floor(s / 60) + ':' + String(s % 60).padStart(2, '0');
};

async function boot() {
  if (!window.__TAURI__ && /[?&]mock=1|#mock/.test(location.search + location.hash)) {
    await new Promise((res) => {
      const s = document.createElement('script');
      s.src = 'mock.js';
      s.onload = res; s.onerror = res;
      document.head.append(s);
    });
  }
  T = window.__TAURI__;
  if (!T) {
    ui.notice = 'Backend non disponibile (apri la pagina con ?mock=1 per provarla nel browser).';
    renderBanner();
    return;
  }
  T.event.listen('state', (e) => render(e.payload));
  T.event.listen('speedtest', (e) => {
    ui.speed.progress = e.payload.progress; ui.speed.mbps = e.payload.mbps;
    updateSpeedProgress();
  });
  setInterval(tickElapsed, 1000);
  try { render(await invoke('get_state')); } catch (e) { ui.notice = errMsg(e); renderBanner(); }
  try { ui.set = await invoke('get_settings'); render(snap); } catch (_) {}
}

function viewFor(state) {
  if (!state) return 'loading';
  if (ui.view === 'settings') return 'settings';
  if (state.capture && state.capture.state !== 'ready') return 'install';
  if (!state.auth || !state.auth.logged_in) return 'login';
  return state.match ? 'match' : 'home';
}

function currentView() { return viewFor(snap); }

function visibleView() {
  return ['loading', 'login', 'install', 'home', 'match', 'settings'].find((v) => !$('v-' + v).hidden) || 'loading';
}

function transition(update, direction = 'forward') {
  const reduced = matchMedia('(prefers-reduced-motion: reduce)').matches;
  if (!document.startViewTransition || reduced || document.hidden) {
    update();
    return null;
  }
  document.documentElement.dataset.transition = direction;
  const movement = document.startViewTransition(update);
  movement.finished.finally(() => delete document.documentElement.dataset.transition);
  return movement;
}

function render(s) {
  const from = visibleView();
  const to = viewFor(s);
  const phaseChanged = from === 'match' && to === 'match' && snap && snap.match && s.match && snap.match.phase !== s.match.phase;
  if ((from !== to || phaseChanged) && snap) {
    const direction = to === 'settings' ? 'settings' : from === 'settings' ? 'back' : 'forward';
    transition(() => renderNow(s), direction);
    return;
  }
  renderNow(s);
}

function renderNow(s) {
  snap = s;
  if (!s) return;
  const m = s.match;
  if (m && m.can_start) ui.forceArmed = false;
  const view = currentView();
  document.body.dataset.view = view;
  for (const v of ['loading', 'login', 'install', 'home', 'match', 'settings']) $('v-' + v).hidden = v !== view;
  $('hd-main').hidden = view === 'settings';
  $('hd-set').hidden = view !== 'settings';
  $('who').textContent = s.auth && s.auth.logged_in ? s.auth.name || '' : '';
  renderConn();
  renderBanner();
  if (view === 'install') renderInstall();
  if (view === 'login') renderLogin();
  if (view === 'home') {
    if (ui.prevView !== 'home') loadRecents();
    renderHome();
  }
  if (view === 'match') renderMatch();
  ui.prevView = view;
}

function renderConn() {
  const c = snap.connection;
  region('conn', String(c), () => {
    if (c === 'reconnecting') return [h('div', { class: 'reconnecting', text: 'Riconnessione…' })];
    if (c === 'offline') return [h('div', { class: 'offline', text: 'Offline' })];
    return [];
  });
}

function renderBanner() {
  const err = ui.notice || (snap && snap.error && snap.error !== ui.dismissedErr ? snap.error : null);
  const upd = snap && snap.update && snap.update.ready ? snap.update : null;
  const ff = snap && snap.ffmpeg && snap.ffmpeg.state !== 'ready' ? snap.ffmpeg : null;
  region('banner', JSON.stringify([err, upd && upd.version, ui.busy === 'applyUpdate', !!(snap && snap.match), ff && [ff.state, Math.round((ff.progress || 0) * 100), ff.error]]), () => [
    ...(ff ? [h('div', { class: 'banner' },
      h('span', { text: ff.state === 'error'
        ? 'Non riesco a preparare ffmpeg: ' + (ff.error || 'errore') + '. Riprovo da solo.'
        : 'Preparo i componenti per registrare' + (ff.state === 'installing' && ff.progress > 0 ? ' · ' + Math.round(ff.progress * 100) + '%' : '…') }),
      ...(ff.state === 'error' ? [h('button', { class: 'btn primary sm', 'data-act': 'retryFfmpeg', text: 'Riprova' })] : []))] : []),

    ...(upd ? [h('div', { class: 'banner update' },
      h('span', { text: snap.match ? 'Aggiornamento ' + upd.version + ' pronto: si installa quando esci dalla partita' : 'Aggiornamento ' + upd.version + ' pronto' }),
      ...(snap.match ? [] : [h('button', { class: 'btn primary sm', 'data-act': 'applyUpdate', disabled: !!ui.busy, text: ui.busy === 'applyUpdate' ? 'Aggiorno…' : 'Riavvia e aggiorna' })]))] : []),
    ...(err ? [h('div', { class: 'banner', role: 'alert' },
      h('span', { text: err }),
      h('button', { class: 'iconbtn', 'data-act': 'dismiss', 'aria-label': 'Chiudi avviso' }, ico('close', 14)))] : []),
  ]);
}

function renderLogin() {
  region('v-login', JSON.stringify([ui.busy === 'login', ui.loginErr]), () => [
    h('div', { class: 'hero-login' },
      h('div', { class: 'bigdot', 'aria-hidden': 'true' }),
      h('h1', { text: 'La partita finisce.\nIl replay resta.' }),
      h('p', { text: 'Registra la tua visuale di gioco e ritrovala nel replay della partita.' }),
      h('button', { class: 'btn primary lg', 'data-act': 'login', disabled: ui.busy === 'login', text: ui.busy === 'login' ? 'Apro il browser…' : 'Accedi con GavaAuth' }),
      ui.loginErr && h('p', { class: 'err', role: 'alert', text: ui.loginErr })),
  ]);
}

function installStageLabel(c) {
  if (c.state === 'error') return 'Non riesco a preparare la registrazione';
  if (c.stage === 'exe') return 'Scarico il programma di registrazione';
  if (c.stage === 'check') return 'Controllo che questo PC riesca a registrare';
  return 'Scarico i componenti per registrare';
}

function encoderLabel(id) {
  return { nvenc: 'scheda NVIDIA', amf: 'scheda AMD', qsv: 'grafica Intel', x264: 'processore', software: 'processore' }[id] || id;
}

function captureInfo() {
  const c = snap && snap.capture && snap.capture.compat;
  if (!c || !c.ok) return null;
  return h('div', null,
    c.best_encoder && h('p', { class: 'hint', text: 'Codifica con ' + encoderLabel(c.best_encoder) + '.' }),
    c.yellow_border && h('p', { class: 'hint warn', text: 'Su questa versione di Windows compare un bordo giallo attorno al gioco mentre registra: lo disegna Windows e non finisce nel video.' }));
}

function renderInstall() {
  region('v-install', JSON.stringify(snap.capture), () => {
    const c = snap.capture;
    const compat = c.compat;
    return [
      h('div', { class: 'hero-login' },
        h('div', { class: 'bigdot', 'aria-hidden': 'true' }),
        h('h1', { text: 'Relay' }),
        h('p', { text: installStageLabel(c) }),
        c.state !== 'error' && h('div', { class: 'bar' }, h('i', { style: 'width:' + Math.round((c.percent || 0) * 100) + '%' })),
        compat && !compat.ok && h('p', { class: 'err', role: 'alert', text: compat.error || 'Questo PC non e’ compatibile con la registrazione.' }),
        c.state === 'error' && !compat && h('p', { class: 'err', role: 'alert', text: c.error }),
        c.state === 'error' && h('button', { class: 'btn primary lg', 'data-act': 'retryCapture', text: 'Riprova' }),
        c.state === 'error' && h('button', { class: 'linkbtn', 'data-act': 'skipCapture', text: 'Salta il controllo (a tuo rischio)' }),
        c.state === 'error' && h('p', { class: 'hint warn', text: 'Se lo salti, potresti non riuscire a registrare, o farlo con qualita’ scarsa.' })),
    ];
  });
}

async function loadRecents() {
  if (ui.recentsLoading) return;
  ui.recentsLoading = true;
  try { ui.recents = (await invoke('list_matches')).slice(0, 10); } catch (_) { ui.recents = []; }
  ui.recentsLoading = false;
  if (snap) renderHome();
  for (const r of ui.recents || []) if (r.status === 'done') ensureThumb(r.id);
}

function ensureThumb(id) {
  if (id in ui.thumbs) return;
  ui.thumbs[id] = null;
  invoke('match_thumb', { id }).then((url) => {
    if (!url || !snap) return;
    ui.thumbs[id] = url;
    if (currentView() === 'match') renderMatch(); else renderHome();
  }).catch(() => {});
}

function renderHome() {
  const inv = snap && snap.invite_request;
  region('v-home', JSON.stringify([ui.recents, ui.busy, ui.joinErr, inv, ui.showJoin, ui.thumbs]), () => [
    ...(inv ? [h('div', { class: 'invitecard', role: 'alert' },
      h('div', { class: 't1', text: 'Invito a una partita' }),
      h('div', { class: 'small muted', text: 'Qualcuno ti ha invitato con un link. Entri solo se sei stato invitato da una persona che conosci.' }),
      h('div', { class: 'row' },
        h('button', { class: 'btn primary', 'data-act': 'acceptInvite', disabled: !!ui.busy, text: ui.busy === 'acceptInvite' ? 'Entro…' : 'Entra' }),
        h('button', { class: 'btn ghost', 'data-act': 'declineInvite', disabled: !!ui.busy, text: 'Ignora' })))] : []),
    h('div', { class: 'card hero-home' },
      h('label', { class: 'label', for: 'new-match-name', text: 'Nome della partita' }),
      h('input', { type: 'text', id: 'new-match-name', 'data-f': 'newName', 'data-k': 'newName', value: ui.newName, maxlength: '60', placeholder: 'Puoi aggiungerlo anche dopo', 'aria-label': 'Nome della partita', autocomplete: 'off', spellcheck: 'false' }),
      h('button', { class: 'btn primary lg block', 'data-act': 'create', disabled: !!ui.busy, text: ui.busy === 'create' ? 'Creo…' : 'Crea partita' }),),

    ui.showJoin || ui.joinErr
      ? h('form', { 'data-form': 'join', style: 'margin-top:14px' },
          h('div', { class: 'pillfield' },
            h('input', { type: 'text', id: 'join-input', 'data-f': 'joinLink', 'data-k': 'joinLink', value: ui.joinLink, placeholder: 'Incolla il link di invito', 'aria-label': 'Link di invito', autocomplete: 'off', spellcheck: 'false' }),
            h('button', { type: 'submit', class: 'iconbtn', disabled: !!ui.busy, 'aria-label': 'Entra' }, ico('arrow', 16))),
          ui.joinErr && h('p', { class: 'err', style: 'margin:6px 2px 0', role: 'alert', text: ui.joinErr }))
      : h('div', { class: 'center', style: 'margin-top:10px' }, h('button', { class: 'linkbtn', 'data-act': 'showJoin', text: 'Entra con un link' })),

    h('div', { class: 'section-heading' }, h('h2', { text: 'Le tue partite' }), h('span', { class: 'small muted', text: 'Recenti' })),
    h('div', { class: 'recents-scroll' },
      ui.recents == null ? h('p', { class: 'empty', text: 'Caricamento…' })
        : !ui.recents.length ? h('p', { class: 'empty', text: 'Nessuna partita ancora.' })
        : h('ul', { class: 'rows' }, ui.recents.map((r) => {
            const thumb = r.status === 'done' ? ui.thumbs[r.id] : null;
            return h('li', null,
              h('button', { class: 'rowbtn' + (thumb ? ' has-thumb' : ''), 'data-act': 'open', 'data-id': r.id, disabled: !!ui.busy },
                h('span', { class: 'dot ' + (r.status === 'recording' ? 'rec' : r.status === 'done' ? 'ok' : '') }),
                h('span', { class: 'txt' },
                  h('div', { class: 't1', text: r.name || (r.players && r.players.length ? r.players.join(', ') : 'Nessun giocatore') }),
                  h('div', { class: 't2', text: (STATUS_MATCH[r.status] || r.status) + (r.host ? ' · host' : '') + ' · ' + new Date(r.created_at * 1000).toLocaleString('it-IT', { day: '2-digit', month: '2-digit', hour: '2-digit', minute: '2-digit' }) })),
                thumb && h('img', { class: 'rowthumb', src: thumb, alt: '', 'aria-hidden': 'true' })));
          })),
    ),
  ]);
}

const PHASE_CHIP = {
  lobby: ['Lobby', ''], recording: ['Rec', 'rec'], stopping: ['Chiusura', 'busy'],
  uploading: ['Caricamento', 'busy'], done: ['Completata', 'ok'], error: ['Errore', 'bad'],
};
const isHostRole = (m) => m.role === 'host' || m.role === 'host_player';
const playsRole = (m) => m.role === 'player' || m.role === 'host_player';
const initial = (name) => (String(name || '?').trim().charAt(0) || '?').toUpperCase();

function playerState(p, phase) {
  if (phase === 'done') return null;
  if (!p.connected) return { label: 'Offline', cls: 'bad', note: 'Non connesso al server' };
  if (p.state === 'error') return { label: 'Errore', cls: 'bad', note: p.issue || 'Errore di registrazione' };
  if (p.state === 'recording') return { label: 'Registra', cls: 'rec', note: p.window };
  if (p.state === 'uploading') return { label: 'Carica', cls: 'busy', note: (p.backlog || 0) + ' pezzi in coda' };
  if (p.ready) return { label: 'Pronto', cls: 'ok', note: p.window };
  return { label: 'Non pronto', cls: 'warn', note: p.issue || p.audio_issue || 'In preparazione' };
}

function playerStats(p) {
  const out = [];
  if (p.rtt_ms != null) out.push('ping ' + p.rtt_ms + ' ms');
  if (p.upload_kbps > 0) out.push((p.upload_kbps / 1000).toFixed(1) + ' Mbit/s');
  if (p.backlog) out.push('coda ' + p.backlog);
  if (p.audio_issue) out.push(p.audio_issue);
  else if (p.audio) out.push('audio: ' + p.audio);
  return out;
}

function fmtDuration(ms) {
  if (!(ms > 0)) return '—';
  const s = Math.round(ms / 1000), hh = Math.floor(s / 3600), mm = Math.floor((s % 3600) / 60), ss = s % 60;
  return hh ? hh + 'h ' + String(mm).padStart(2, '0') + 'm' : mm + 'm ' + String(ss).padStart(2, '0') + 's';
}

function topBlock(m, isHost) {
  const [chip, chipCls] = PHASE_CHIP[m.phase] || [m.phase, ''];
  const canLeave = !['recording', 'stopping', 'uploading'].includes(m.phase);
  const title = ui.renaming && isHost
    ? h('form', { 'data-form': 'rename', class: 'm-rename' },
        h('input', { type: 'text', id: 'rename-input', 'data-f': 'renameDraft', maxlength: '60', value: ui.renameDraft, placeholder: 'Nome della partita', 'aria-label': 'Nome della partita', autocomplete: 'off', spellcheck: 'false' }),
        h('button', { type: 'submit', class: 'btn primary sm', disabled: !!ui.busy, text: 'Salva' }),
        h('button', { type: 'button', class: 'btn ghost sm', 'data-act': 'renameCancel', text: 'Annulla' }))
    : h('div', { class: 'm-titlewrap' },
        h('h1', { class: 'm-title' + (m.name ? '' : ' auto'), title: m.name || '', text: m.name || 'Partita senza nome' }),
        isHost && h('button', { class: 'iconbtn sm', 'data-act': 'renameStart', 'aria-label': 'Rinomina la partita', title: 'Rinomina' }, ico('edit', 14)));
  return [
    h('div', { class: 'm-nav' },
      canLeave
        ? h('button', { class: 'm-back', 'data-act': 'leave', disabled: !!ui.busy }, ico('back', 15), 'Partite')
        : h('span', { class: 'm-back ghosted' }, ico('back', 15), 'Partite')),
    h('div', { class: 'm-headrow' },
      title,
      h('span', { class: 'chip ' + chipCls }, m.phase === 'recording' && h('span', { class: 'rdot' }), chip)),
  ];
}

function readinessMeter(m) {
  const total = m.players.length, ready = m.players.filter((p) => p.ready).length;
  return h('div', { class: 'meter' },
    h('div', { class: 'meter-num' }, h('strong', { text: String(ready) }), h('span', { text: '/' + total })),
    h('div', { class: 'meter-body' },
      h('div', { class: 'meter-label', text: total ? (ready === total ? 'Tutti pronti' : 'giocatori pronti') : 'Nessun giocatore' }),
      h('div', { class: 'segs' }, total
        ? m.players.map((_, i) => h('i', { class: i < ready ? 'on' : '' }))
        : h('i'))));
}

function ctaStart(m) {
  const busy = !!ui.busy, canStart = !!m.can_start;
  return [
    h('button', { class: 'btn primary lg block', 'data-act': 'start', disabled: !canStart || busy }, ico('play', 15), ui.busy === 'start' ? 'Avvio…' : 'Avvia registrazione'),
    !canStart && m.players.length > 0 && h('button', { class: 'linkbtn' + (ui.forceArmed ? ' armed' : ''), 'data-act': 'force', disabled: busy, text: ui.forceArmed ? 'Confermi? Avvia senza aspettare' : 'Avvia comunque' }),
  ];
}

function heroBlock(m, isHost) {
  const busy = !!ui.busy;
  const stopBtn = () => h('button', { class: 'btn danger lg block', 'data-act': 'stop', disabled: busy, text: ui.busy === 'stop' ? 'Fermo…' : 'Ferma registrazione' });

  if (m.phase === 'lobby' && m.started_at_ms != null) {
    return [
      h('div', { class: 'eyebrow live' }, h('span', { class: 'rdot' }), 'PARTITA IN CORSO'),
      h('div', { class: 'h-title', text: playsRole(m) ? 'Riprendi a registrare' : 'Registrazione in corso' }),
      h('p', { class: 'h-sub', text: 'Riparte da solo appena il gioco è aperto e scelto. Il tratto mancante resta scuro.' }),
      isHost && m.stopped_at_ms == null && h('div', { class: 'h-cta' }, stopBtn()),
    ];
  }
  if (m.phase === 'lobby') {
    if (isHost) {
      const blockers = m.players.length ? m.start_blockers || [] : [];
      return [
        h('div', { class: 'eyebrow', text: 'PRIMA DI INIZIARE' }),
        readinessMeter(m),
        !m.players.length && h('p', { class: 'h-sub', text: 'Condividi il link di invito: chi entra compare qui a destra.' }),
        blockers.length > 0 && h('ul', { class: 'blockers', id: 'blk' }, blockers.map((b) => h('li', { text: b }))),
        h('div', { class: 'h-cta' }, ...ctaStart(m)),
      ];
    }
    const me = m.players.find((p) => p.id === snap.auth.user);
    const st = me && playerState(me, m.phase);
    return [
      h('div', { class: 'eyebrow', text: 'IN ATTESA DELL’HOST' }),
      h('div', { class: 'h-title', text: 'Parte quando l’host avvia' }),
      h('p', { class: 'h-sub', text: 'Tieni il gioco aperto e scegli la finestra qui sotto: al resto pensa Relay.' }),
      st && h('div', { class: 'selfstate ' + st.cls }, h('span', { class: 'pill ' + st.cls, text: st.label }), h('span', { class: 'grow', text: st.cls === 'ok' ? 'Sei pronto a registrare' : st.note })),
    ];
  }
  if (m.phase === 'recording') {
    const me = m.me;
    return [
      h('div', { class: 'eyebrow live' }, h('span', { class: 'rdot' }), 'IN REGISTRAZIONE'),
      h('div', { id: 'elapsed', class: 'timer' }),
      me && h('div', { class: 'kpis' },
        h('div', null, h('span', { text: 'Upload' }), h('strong', { text: me.upload_kbps > 0 ? (me.upload_kbps / 1000).toFixed(1) + ' Mbit/s' : '—' })),
        h('div', null, h('span', { text: 'In coda' }), h('strong', { text: String(me.backlog || 0) })),
        h('div', null, h('span', { text: 'Ping' }), h('strong', { text: me.rtt_ms != null ? me.rtt_ms + ' ms' : '—' }))),
      h('div', { class: 'h-cta' }, isHost ? stopBtn() : h('p', { class: 'h-sub', text: 'Tieni aperto il gioco: al resto pensa Relay.' })),
    ];
  }
  if (m.phase === 'stopping' || m.phase === 'uploading') {
    const q = (m.me && m.me.backlog) || 0;
    const max = Math.max(ui.upMax || 0, q);
    const pct = max ? Math.round(((max - q) / max) * 100) : 100;
    return [
      h('div', { class: 'eyebrow', text: 'QUASI FATTO' }),
      h('div', { class: 'h-title', text: m.phase === 'stopping' ? 'Chiudo la registrazione' : 'Carico gli ultimi pezzi' }),
      h('div', { class: 'bar big' }, h('i', { style: 'width:' + pct + '%' })),
      h('p', { class: 'h-sub', text: (q ? q + ' pezzi in coda · ' : '') + 'Non chiudere l’app' }),
    ];
  }
  if (m.phase === 'done') {
    const thumb = ui.thumbs[m.id];
    const dur = m.started_at_ms != null && m.stopped_at_ms != null ? m.stopped_at_ms - m.started_at_ms : null;
    const date = m.created_at ? new Date(m.created_at * 1000).toLocaleString('it-IT', { day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' }) : '—';
    return [
      h('div', { class: 'poster' + (thumb ? '' : ' empty') },
        thumb ? h('img', { src: thumb, alt: 'Anteprima della registrazione' }) : h('span', { class: 'poster-mark', 'aria-hidden': 'true' }),
        m.game && h('span', { class: 'poster-game' }, ico('gamepad', 13), m.game.name),
        h('button', { class: 'poster-play', 'data-act': 'replay', 'aria-label': 'Apri il replay' }, ico('play', 22))),
      h('div', { class: 'h-cta row2' },
        h('button', { class: 'btn primary lg', 'data-act': 'replay' }, ico('external', 15), 'Apri replay'),
        h('button', { class: 'btn ghost lg', 'data-act': 'newmatch', disabled: busy, text: 'Nuova partita' })),
      h('div', { class: 'facts' },
        h('div', null, h('span', { text: 'Data' }), h('strong', { text: date })),
        h('div', null, h('span', { text: 'Durata' }), h('strong', { text: fmtDuration(dur) })),
        h('div', null, h('span', { text: 'Giocatori' }), h('strong', { text: String(m.players.length) }))),
      m.fnf && h('div', { class: 'facts fnf' },
        h('div', { class: 'wide' }, h('span', { text: 'Canzone' }), h('strong', { text: m.fnf.song + (m.fnf.difficulty ? ' · ' + m.fnf.difficulty : '') })),
        m.fnf.accuracy != null && h('div', null, h('span', { text: 'Accuracy' }), h('strong', { text: (m.fnf.accuracy * 100).toFixed(1) + '%' })),
        h('div', null, h('span', { text: 'Note mancate' }), h('strong', { text: String(m.fnf.misses || 0) })),
        m.fnf.score != null && h('div', null, h('span', { text: 'Punteggio' }), h('strong', { text: Number(m.fnf.score).toLocaleString('it-IT') }))),
    ];
  }
  if (m.phase === 'error') {
    return [
      h('div', { class: 'eyebrow', text: 'ERRORE' }),
      h('div', { class: 'h-title', text: 'Qualcosa non va' }),
      h('p', { class: 'err', text: snap.error || 'Si è verificato un errore.' }),
      h('div', { class: 'h-cta' }, h('button', { class: 'btn ghost lg block', 'data-act': 'leave', disabled: busy, text: 'Esci dalla partita' })),
    ];
  }
  return [];
}

function peopleBlock(m, isHost) {
  const me = snap.auth.user;
  const items = m.players.map((p) => {
    const st = playerState(p, m.phase);
    const stats = st ? playerStats(p) : [];
    const open = ui.open.has(p.id);
    const host = p.is_host || p.host;
    return h('li', null,
      h('button', { class: 'person' + (open ? ' open' : ''), 'data-act': 'toggle', 'data-id': p.id, 'data-k': 'p-' + p.id, 'aria-expanded': String(open), disabled: !st },
        h('span', { class: 'avatar sm ' + (st ? st.cls : '') }, initial(p.name)),
        h('span', { class: 'txt' },
          h('span', { class: 'pname' }, p.name, p.id === me && h('em', { text: 'tu' }), host && h('em', { text: 'host' })),
          st && st.note && h('span', { class: 'pnote ' + (st.cls === 'ok' || st.cls === 'rec' ? '' : st.cls), title: st.note, text: st.note })),
        st && h('span', { class: 'pill ' + st.cls, text: st.label })),
      open && stats.length > 0 && h('div', { class: 'pstats', text: stats.join(' · ') }));
  });
  return [
    h('div', { class: 'card-head' }, h('h2', { text: m.phase === 'done' ? 'Chi ha giocato' : 'Giocatori' }), h('span', { class: 'count', text: String(items.length) })),
    items.length
      ? h('ul', { class: 'people', 'aria-label': 'Giocatori' }, items)
      : h('p', { class: 'empty', text: isHost ? 'Ancora nessuno. Condividi il link di invito.' : 'In attesa dei giocatori…' }),
  ];
}

function inviteBlock(m) {
  return [h('div', { class: 'm-card invite' },
    h('div', { class: 'card-head' }, h('h2', { text: 'Invita' }), h('span', { class: 'small muted', text: 'chi ha il link entra' })),
    h('div', { class: 'linkbox' },
      h('span', { class: 'linktext', title: m.invite_url, text: m.invite_url.replace(/^https?:\/\//, '') }),
      h('button', { class: 'btn sm' + (ui.copied ? ' primary' : ''), 'data-act': 'copy', 'data-k': 'copy' }, ico(ui.copied ? 'check' : 'copy', 13), ui.copied ? 'Copiato' : 'Copia')),
    h('input', { type: 'text', class: 'sr', id: 'invite-input', readOnly: true, value: m.invite_url, 'aria-label': 'Link di invito', tabindex: '-1' }),
    h('p', { class: 'hint', text: 'Chiedi a tutti di scegliere la propria finestra prima di iniziare.' }))];
}

function renderMatch() {
  const m = snap.match;
  const isHost = isHostRole(m);
  $('v-match').dataset.phase = m.phase;
  $('v-match').dataset.role = isHost ? 'host' : 'player';
  if (m.phase === 'stopping' || m.phase === 'uploading') ui.upMax = Math.max(ui.upMax || 0, (m.me && m.me.backlog) || 0);
  else ui.upMax = 0;
  if (m.phase === 'done') ensureThumb(m.id);

  region('m-top', JSON.stringify([m.name, m.phase, m.role, ui.renaming, ui.busy]), () => topBlock(m, isHost));
  region('m-hero', JSON.stringify([m.phase, m.role, m.players.map((p) => [p.name, p.ready, p.issue, p.connected, p.window, p.state]), m.me, m.started_at_ms != null, m.stopped_at_ms, m.can_start, m.start_blockers, ui.forceArmed, ui.busy, ui.thumbs[m.id] || null, m.game, m.fnf, m.created_at, ui.upMax]), () => heroBlock(m, isHost));
  tickElapsed();

  renderWindowPicker(m);

  const showInvite = isHost && m.phase === 'lobby' && m.started_at_ms == null && !!m.invite_url;
  region('m-invite', JSON.stringify([showInvite && m.invite_url, ui.copied]), () => (showInvite ? inviteBlock(m) : []));
  region('m-players', JSON.stringify([m.players, m.phase, snap.auth.user, isHost, [...ui.open]]), () => peopleBlock(m, isHost));
}

function renderWindowPicker(m) {
  const show = playsRole(m) && m.phase === 'lobby' && !!ui.set;
  if (show && ui.windows == null && !ui.winBusy) refreshWindows();
  if (show && !ui.steamChecked) refreshSteam();
  region('m-window', JSON.stringify([show, ui.windows, ui.set && ui.set.window, ui.winBusy, ui.winErr, ui.steam, ui.game, ui.gameQuery, ui.gameResults, ui.gameBusy, ui.set && [ui.set.audio_game, ui.set.audio_mic, ui.set.audio_mic_gain]]), () => {
    if (!show) return [];
    const w = ui.set.window;
    const list = ui.windows || [];
    const both = w ? list.findIndex((x) => x.exe === w.exe && x.title === w.title) : -1;
    const idx = w ? (both >= 0 ? both : list.findIndex((x) => x.exe === w.exe)) : -1;
    const selVal = !w ? '' : idx >= 0 ? String(idx) : 'cur';
    const isMon = (x) => !!x && String(x.exe).startsWith('@monitor:');
    const monitor = isMon(w);
    const label = (x) => (isMon(x) ? x.title : x.title + ' — ' + x.exe);
    const opts = [h('option', { value: '', text: 'Scegli cosa registrare…', selected: selVal === '' })];
    if (w && idx < 0) opts.push(h('option', { value: 'cur', text: label(w) + ' (non trovata)', selected: true }));
    list.forEach((x, i) => opts.push(h('option', { value: String(i), text: label(x), selected: selVal === String(i) })));

    const g = ui.game;
    const detected = ui.steam && !(g && g.name === ui.steam.name && g.app_id === ui.steam.app_id) ? ui.steam : null;
    const micPct = Math.round((ui.set.audio_mic_gain || 3) * 100);

    return [h('div', { class: 'm-card setup' },
      h('div', { class: 'card-head' }, h('h2', { text: 'La tua registrazione' })),

      h('div', { class: 'srow' },
        h('span', { class: 'sico' }, ico('monitor', 16)),
        h('div', { class: 'sbody' },
          h('div', { class: 'slabel', text: 'Cosa registro' }),
          h('div', { class: 'row' },
            h('select', { class: 'grow', 'data-f': 'window', 'data-k': 'window', 'aria-label': 'Finestra da registrare', title: w && !monitor ? 'Riconosciuta dal programma: ' + w.exe : '' }, opts),
            h('button', { class: 'iconbtn', 'data-act': 'refresh-windows', disabled: ui.winBusy, 'aria-label': 'Aggiorna elenco', title: 'Aggiorna elenco' }, ico('refresh', 15))),
          ui.winErr && h('p', { class: 'err', text: ui.winErr }),
          monitor && h('p', { class: 'hint warn', text: 'Registra tutto lo schermo, notifiche comprese.' }))),

      h('div', { class: 'srow' },
        h('span', { class: 'sico' }, ico('gamepad', 16)),
        h('div', { class: 'sbody' },
          h('div', { class: 'slabel', text: 'Gioco nel replay' }),
          g
            ? h('div', { class: 'gamepick' }, h('span', { class: 'grow', text: g.name }), h('button', { class: 'linkbtn', 'data-act': 'clearGame', text: 'Cambia' }))
            : h('input', { type: 'text', 'data-f': 'game-query', 'data-k': 'game-query', value: ui.gameQuery, placeholder: 'Cerca il gioco…', 'aria-label': 'Cerca nel catalogo dei giochi', autocomplete: 'off', spellcheck: 'false' }),
          !g && detected && h('button', { class: 'suggest', 'data-act': 'pickGame', 'data-i': 'steam' }, h('span', { class: 'grow', text: detected.name }), h('span', { class: 'small muted', text: 'aperto su Steam' })),
          !g && ui.gameBusy && h('p', { class: 'hint', text: 'Cerco…' }),
          !g && ui.gameResults.length > 0 && h('div', { class: 'results' }, ui.gameResults.slice(0, 5).map((r, i) => h('button', { class: 'suggest', 'data-act': 'pickGame', 'data-i': String(i), text: r.name }))),
          h('p', { class: 'hint', text: 'Serve solo per titolo e copertina del replay.' }))),

      h('div', { class: 'srow' },
        h('span', { class: 'sico' }, ico('speaker', 16)),
        h('div', { class: 'sbody' },
          h('div', { class: 'slabel', text: 'Audio' }),
          h('div', { class: 'audiosw' },
            h('label', { class: 'switch' }, h('input', { type: 'checkbox', 'data-f': 'audio_game', 'data-k': 'audio_game', checked: !!ui.set.audio_game }), h('span', { class: 'track' }), h('span', { text: 'Gioco' })),
            h('label', { class: 'switch' }, h('input', { type: 'checkbox', 'data-f': 'audio_mic', 'data-k': 'audio_mic', checked: !!ui.set.audio_mic }), h('span', { class: 'track' }), h('span', { text: 'Microfono' }))),
          ui.set.audio_mic && h('div', { class: 'micgain' },
            h('div', { class: 'row' }, h('span', { class: 'small muted grow', text: 'Volume microfono' }), h('span', { id: 'mic-gain-val', class: 'small', text: micPct + '%' })),
            h('input', { type: 'range', min: '50', max: '800', step: '25', 'data-f': 'audio_mic_gain', 'data-k': 'audio_mic_gain', value: String(micPct), 'aria-label': 'Volume del microfono', title: 'Se nel replay non si sente, alzalo. Vale dalla prossima registrazione.' }))))
    )];
  });
}

function tickElapsed() {
  const el = $('elapsed');
  if (!el || !snap || !snap.match) return;
  const m = snap.match;
  el.textContent = m.phase === 'recording' && m.started_at_ms != null
    ? fmtTime(Date.now() + (m.clock_offset_ms ?? 0) - m.started_at_ms) : '';
}

const SET_PAGES = {
  main: ['Impostazioni', () => buildSettings()],
  connectors: ['Connettori', () => buildConnectors()],
  cne: ['Codename Engine', () => buildCne()],
  funkin: ['Friday Night Funkin’', () => buildFunkin()],
  psych: ['Psych Engine', () => buildPsych()],
  nmv: ['Nightmare Vision', () => buildNmv()],
  kade: ['Kade Engine', () => buildKade()],
  gd: ['Geometry Dash', () => buildGd()],
};
const setRev = () => {
  const page = SET_PAGES[ui.setPage] ? ui.setPage : 'main';
  $('v-settings').dataset.page = page;
  $('set-title').textContent = SET_PAGES[page][0];
  region('v-settings', String(++ui.rev), SET_PAGES[page][1]);
  updateSaveMsg();
};
function goSetPage(page, direction) {
  transition(() => { ui.setPage = page; setRev(); $('v-settings').scrollTop = 0; }, direction);
}
const CNE_LOGO = 'img/codename-engine.png';
const FNF_LOGO = 'img/fnf.png';
const PSYCH_LOGO = 'img/psych-engine.png';
const NMV_LOGO = 'img/nightmare-vision.png';
const KADE_LOGO = 'img/kade-engine.png';
const GD_LOGO = 'img/geometry-dash.png';
const folderName = (p) => {
  const parts = String(p).replace(/[\\/]+$/, '').split(/[\\/]/);
  const last = parts.pop() || p;
  return last.toLowerCase() === 'bin' && parts.length ? parts.pop() : last;
};

const ENGINE_NAMES = { funkin: 'Friday Night Funkin’', codename: 'Codename Engine', psych: 'Psych Engine', nmv: 'Nightmare Vision', kade: 'Kade Engine', gd: 'Geometry Dash' };

function scanBlock() {
  const r = ui.scanResult;
  const fresh = r ? r.found.filter((f) => f.status === 'nuova') : [];
  const failed = r ? r.found.filter((f) => f.status === 'errore') : [];
  return h('div', { class: 'scanbox' },
    h('div', { class: 'scanbox-head' },
      h('div', null,
        h('strong', { text: 'Cerca le mod sul PC' }),
        h('p', { class: 'hint', text: 'Relay controlla i tuoi dischi, riconosce il motore di ogni gioco e collega da solo le mod compatibili.' })),
      h('button', { class: 'btn primary', 'data-act': 'scanMods', disabled: ui.scanBusy, text: ui.scanBusy ? 'Cerco…' : 'Cerca' })),
    ui.scanBusy && h('p', { class: 'hint', text: 'Può volerci qualche minuto: puoi continuare a usare Relay.' }),
    ui.scanErr && h('p', { class: 'err', role: 'alert', text: ui.scanErr }),
    r && h('div', { class: 'scanres' },
      h('p', { class: 't1', text: r.found.length
        ? `Trovate ${r.found.length} mod compatibili: ${fresh.length ? fresh.length + (fresh.length === 1 ? ' collegata ora' : ' collegate ora') : 'erano già tutte collegate'}.`
        : 'Nessuna mod compatibile trovata.' }),
      fresh.length > 0 && h('ul', { class: 'modlist' }, fresh.map((f) => h('li', null,
        h('span', { class: 'txt' }, h('span', { class: 'modname', text: f.name }), h('span', { class: 'modpath', title: f.folder, text: ENGINE_NAMES[f.engine] + ' · ' + f.folder }))))),
      failed.length > 0 && h('p', { class: 'hint', text: `Non collegabili (${failed.length}): ` + failed.map((f) => f.name).join(', ') + '. Non caricano script esterni.' }),
      r.other_engines > 0 && h('p', { class: 'hint', text: `Altri ${r.other_engines} giochi usano motori non ancora supportati.` })));
}

function buildConnectors() {
  const n = (ui.fnfFolders || []).length;
  const fn = (ui.funkinFolders || []).length;
  const pn = (ui.psychFolders || []).length;
  const nn = (ui.nmvFolders || []).length;
  const kn = (ui.kadeFolders || []).length;
  const gn = (ui.gdFolders || []).length;
  return [
    h('p', { class: 'hint conn-intro', text: 'Collega Relay ai giochi e ai motori che usi: le informazioni della partita finiscono nel replay in automatico.' }),
    scanBlock(),
    h('div', { class: 'ctiles' },
      h('button', { class: 'ctile' + (n ? ' on' : ''), 'data-act': 'openCne' },
        h('span', { class: 'ctile-logo' }, h('img', { src: CNE_LOGO, alt: '' })),
        h('span', { class: 'ctile-name', text: 'Codename Engine' }),
        h('span', { class: 'ctile-state', text: n ? (n === 1 ? '1 mod collegata' : n + ' mod collegate') : 'Non collegato' })),
      h('button', { class: 'ctile' + (fn ? ' on' : ''), 'data-act': 'openFunkin' },
        h('span', { class: 'ctile-logo' }, h('img', { src: FNF_LOGO, alt: '' })),
        h('span', { class: 'ctile-name', text: 'Friday Night Funkin’' }),
        h('span', { class: 'ctile-state', text: fn ? 'Gioco collegato' : 'Non collegato' })),
      h('button', { class: 'ctile' + (pn ? ' on' : ''), 'data-act': 'openPsych' },
        h('span', { class: 'ctile-logo' }, h('img', { src: PSYCH_LOGO, alt: '' })),
        h('span', { class: 'ctile-name', text: 'Psych Engine' }),
        h('span', { class: 'ctile-state', text: pn ? (pn === 1 ? '1 mod collegata' : pn + ' mod collegate') : 'Non collegato' })),
      h('button', { class: 'ctile' + (nn ? ' on' : ''), 'data-act': 'openNmv' },
        h('span', { class: 'ctile-logo' }, h('img', { src: NMV_LOGO, alt: '' })),
        h('span', { class: 'ctile-name', text: 'Nightmare Vision' }),
        h('span', { class: 'ctile-state', text: nn ? (nn === 1 ? '1 mod collegata' : nn + ' mod collegate') : 'Non collegato' })),
      h('button', { class: 'ctile' + (kn ? ' on' : ''), 'data-act': 'openKade' },
        h('span', { class: 'ctile-logo' }, h('img', { src: KADE_LOGO, alt: '' })),
        h('span', { class: 'ctile-name', text: 'Kade Engine' }),
        h('span', { class: 'ctile-state', text: kn ? (kn === 1 ? '1 mod collegata' : kn + ' mod collegate') : 'Non collegato' })),
      h('button', { class: 'ctile' + (gn ? ' on' : ''), 'data-act': 'openGd' },
        h('span', { class: 'ctile-logo' }, h('img', { src: GD_LOGO, alt: '' })),
        h('span', { class: 'ctile-name', text: 'Geometry Dash' }),
        h('span', { class: 'ctile-state', text: gn ? 'Gioco collegato' : 'Non collegato' }))),
  ];
}

function buildGd() {
  const list = ui.gdFolders || [];
  return [
    h('div', { class: 'cne-head' },
      h('span', { class: 'ctile-logo lg' }, h('img', { src: GD_LOGO, alt: '' })),
      h('div', null,
        h('h2', { text: 'Geometry Dash' }),
        h('p', { class: 'hint', text: 'Ogni nuovo record (percentuale, monete o tempo nei platformer) diventa una clip sul sito, con il tuo profilo del gioco.' }))),
    ui.set && h('div', { class: 'cne-opts' },
      h('label', { class: 'switch' }, h('input', { type: 'checkbox', 'data-f': 'fnf_autorecord', 'data-k': 'fnf_autorecord', checked: ui.set.fnf_autorecord !== false }), h('span', { class: 'track' }),
        h('span', { class: 'txt' }, h('span', { class: 't1', text: 'Registra i record in automatico' }), h('span', { class: 't2', text: 'Salva la clip del tentativo solo quando batti il tuo record. Pratica, start position e speedhack non contano.' }))),
      h('label', { class: 'switch' }, h('input', { type: 'checkbox', 'data-f': 'fnf_mic', 'data-k': 'fnf_mic', checked: !!ui.set.fnf_mic, disabled: ui.set.fnf_autorecord === false }), h('span', { class: 'track' }),
        h('span', { class: 'txt' }, h('span', { class: 't1', text: 'Microfono nelle clip' }), h('span', { class: 't2', text: 'Registra anche la tua voce mentre giochi.' })))),
    h('div', { class: 'card-head cne-listhead' }, h('h2', { text: 'Gioco collegato' }), h('span', { class: 'count', text: String(list.length) })),
    list.length
      ? h('ul', { class: 'modlist' }, list.map((f) => h('li', null,
          h('span', { class: 'txt' }, h('span', { class: 'modname', text: 'Geometry Dash' }), h('span', { class: 'modpath', title: f, text: f })),
          h('button', { class: 'btn ghost sm', 'data-act': 'gdRemove', 'data-folder': f, disabled: ui.fnfBusy, text: 'Rimuovi' }))))
      : h('p', { class: 'empty', text: 'Gioco non collegato. Serve Geode (geode-sdk.org): Relay aggiunge la sua mod e la tiene aggiornata.' }),
    !list.length && h('button', { class: 'btn primary lg block', 'data-act': 'gdAdd', disabled: ui.fnfBusy, text: ui.fnfBusy ? 'Collego…' : 'Collega Geometry Dash' }),
    !list.length && h('button', { class: 'btn ghost block', 'data-act': 'gdPick', disabled: ui.fnfBusy, text: 'Scegli la cartella a mano' }),
    ui.fnfErr && h('p', { class: 'err', role: 'alert', text: ui.fnfErr }),
  ];
}

function buildKade() {
  const list = ui.kadeFolders || [];
  return [
    h('div', { class: 'cne-head' },
      h('span', { class: 'ctile-logo lg' }, h('img', { src: KADE_LOGO, alt: '' })),
      h('div', null,
        h('h2', { text: 'Kade Engine' }),
        h('p', { class: 'hint', text: 'Le mod fatte con Kade Engine che usano i modchart Lua, come Indie Cross: ogni record diventa una clip sul sito.' }))),
    recordOpts(),
    h('div', { class: 'card-head cne-listhead' }, h('h2', { text: 'Mod collegate' }), h('span', { class: 'count', text: String(list.length) })),
    list.length
      ? h('ul', { class: 'modlist' }, list.map((f) => h('li', null,
          h('span', { class: 'txt' }, h('span', { class: 'modname', text: folderName(f) }), h('span', { class: 'modpath', title: f, text: f })),
          h('button', { class: 'btn ghost sm', 'data-act': 'kadeRemove', 'data-folder': f, disabled: ui.fnfBusy, text: 'Rimuovi' }))))
      : h('p', { class: 'empty', text: 'Nessuna mod collegata. Scegli la cartella di una mod fatta con Kade Engine (quella con il suo .exe).' }),
    h('button', { class: 'btn primary lg block', 'data-act': 'kadeAdd', disabled: ui.fnfBusy, text: ui.fnfBusy ? 'Collego…' : 'Aggiungi una mod' }),
    ui.fnfErr && h('p', { class: 'err', role: 'alert', text: ui.fnfErr }),
    h('p', { class: 'hint', text: 'Relay mette un piccolo script (modchart.lua) nelle cartelle delle canzoni che non ne hanno già uno e lo tiene aggiornato. Rimuovendo la mod dalla lista viene tolto.' }),
  ];
}

function buildNmv() {
  const list = ui.nmvFolders || [];
  return [
    h('div', { class: 'cne-head' },
      h('span', { class: 'ctile-logo lg' }, h('img', { src: NMV_LOGO, alt: '' })),
      h('div', null,
        h('h2', { text: 'Nightmare Vision' }),
        h('p', { class: 'hint', text: 'Le mod fatte con Nightmare Vision: ogni record diventa una clip sul sito, con settimane e tracklist.' }))),
    recordOpts(),
    h('div', { class: 'card-head cne-listhead' }, h('h2', { text: 'Mod collegate' }), h('span', { class: 'count', text: String(list.length) })),
    list.length
      ? h('ul', { class: 'modlist' }, list.map((f) => h('li', null,
          h('span', { class: 'txt' }, h('span', { class: 'modname', text: folderName(f) }), h('span', { class: 'modpath', title: f, text: f })),
          h('button', { class: 'btn ghost sm', 'data-act': 'nmvRemove', 'data-folder': f, disabled: ui.fnfBusy, text: 'Rimuovi' }))))
      : h('p', { class: 'empty', text: 'Nessuna mod collegata. Scegli la cartella di una mod fatta con Nightmare Vision (quella con il suo .exe).' }),
    h('button', { class: 'btn primary lg block', 'data-act': 'nmvAdd', disabled: ui.fnfBusy, text: ui.fnfBusy ? 'Collego…' : 'Aggiungi una mod' }),
    ui.fnfErr && h('p', { class: 'err', role: 'alert', text: ui.fnfErr }),
    h('p', { class: 'hint', text: 'Relay aggiunge un piccolo script nella cartella content della mod e lo tiene aggiornato. Rimuovendo la mod dalla lista viene tolto.' }),
  ];
}

function buildPsych() {
  const list = ui.psychFolders || [];
  return [
    h('div', { class: 'cne-head' },
      h('span', { class: 'ctile-logo lg' }, h('img', { src: PSYCH_LOGO, alt: '' })),
      h('div', null,
        h('h2', { text: 'Psych Engine' }),
        h('p', { class: 'hint', text: 'Le mod fatte con Psych Engine, vecchie e nuove: ogni record diventa una clip sul sito, con settimane e tracklist.' }))),
    recordOpts(),
    h('div', { class: 'card-head cne-listhead' }, h('h2', { text: 'Mod collegate' }), h('span', { class: 'count', text: String(list.length) })),
    list.length
      ? h('ul', { class: 'modlist' }, list.map((f) => h('li', null,
          h('span', { class: 'txt' }, h('span', { class: 'modname', text: folderName(f) }), h('span', { class: 'modpath', title: f, text: f })),
          h('button', { class: 'btn ghost sm', 'data-act': 'psychRemove', 'data-folder': f, disabled: ui.fnfBusy, text: 'Rimuovi' }))))
      : h('p', { class: 'empty', text: 'Nessuna mod collegata. Scegli la cartella di una mod fatta con Psych Engine (quella con il suo .exe).' }),
    h('button', { class: 'btn primary lg block', 'data-act': 'psychAdd', disabled: ui.fnfBusy, text: ui.fnfBusy ? 'Collego…' : 'Aggiungi una mod' }),
    ui.fnfErr && h('p', { class: 'err', role: 'alert', text: ui.fnfErr }),
    h('p', { class: 'hint', text: 'Relay aggiunge un piccolo script nella cartella mods della mod e lo tiene aggiornato. Rimuovendo la mod dalla lista viene tolto.' }),
  ];
}

const recordOpts = () => ui.set && h('div', { class: 'cne-opts' },
  h('label', { class: 'switch' }, h('input', { type: 'checkbox', 'data-f': 'fnf_autorecord', 'data-k': 'fnf_autorecord', checked: ui.set.fnf_autorecord !== false }), h('span', { class: 'track' }),
    h('span', { class: 'txt' }, h('span', { class: 't1', text: 'Registra le canzoni in automatico' }), h('span', { class: 't2', text: 'Salva la clip solo quando batti il tuo record: la trovi sul sito in Giochi.' }))),
  h('label', { class: 'switch' }, h('input', { type: 'checkbox', 'data-f': 'fnf_mic', 'data-k': 'fnf_mic', checked: !!ui.set.fnf_mic, disabled: ui.set.fnf_autorecord === false }), h('span', { class: 'track' }),
    h('span', { class: 'txt' }, h('span', { class: 't1', text: 'Microfono nelle clip' }), h('span', { class: 't2', text: 'Registra anche la tua voce mentre suoni.' }))));

function buildFunkin() {
  const list = ui.funkinFolders || [];
  return [
    h('div', { class: 'cne-head' },
      h('span', { class: 'ctile-logo lg' }, h('img', { src: FNF_LOGO, alt: '' })),
      h('div', null,
        h('h2', { text: 'Friday Night Funkin’' }),
        h('p', { class: 'hint', text: 'Il gioco originale e tutte le sue mod: ogni record diventa una clip sul sito, con album e tracklist.' }))),
    recordOpts(),
    h('div', { class: 'card-head cne-listhead' }, h('h2', { text: 'Gioco collegato' }), h('span', { class: 'count', text: String(list.length) })),
    list.length
      ? h('ul', { class: 'modlist' }, list.map((f) => h('li', null,
          h('span', { class: 'txt' }, h('span', { class: 'modname', text: folderName(f) }), h('span', { class: 'modpath', title: f, text: f })),
          h('button', { class: 'btn ghost sm', 'data-act': 'funkinRemove', 'data-folder': f, disabled: ui.fnfBusy, text: 'Rimuovi' }))))
      : h('p', { class: 'empty', text: 'Nessun gioco collegato. Scegli la cartella di Friday Night Funkin’ (quella con Funkin.exe).' }),
    !list.length && h('button', { class: 'btn primary lg block', 'data-act': 'funkinAdd', disabled: ui.fnfBusy, text: ui.fnfBusy ? 'Collego…' : 'Scegli la cartella del gioco' }),
    ui.fnfErr && h('p', { class: 'err', role: 'alert', text: ui.fnfErr }),
    h('p', { class: 'hint', text: 'Relay aggiunge una piccola mod nella cartella mods e la tiene aggiornata da sola, anche quando il gioco o le mod si aggiornano. Rimuovendo il gioco dalla lista viene tolta.' }),
  ];
}

function buildCne() {
  const list = ui.fnfFolders || [];
  return [
    h('div', { class: 'cne-head' },
      h('span', { class: 'ctile-logo lg' }, h('img', { src: CNE_LOGO, alt: '' })),
      h('div', null,
        h('h2', { text: 'Codename Engine' }),
        h('p', { class: 'hint', text: 'Friday Night Funkin’: canzone, accuracy e note mancate compaiono nel replay della partita.' }))),
    recordOpts(),
    h('div', { class: 'card-head cne-listhead' }, h('h2', { text: 'Mod collegate' }), h('span', { class: 'count', text: String(list.length) })),
    list.length
      ? h('ul', { class: 'modlist' }, list.map((f) => h('li', null,
          h('span', { class: 'txt' }, h('span', { class: 'modname', text: folderName(f) }), h('span', { class: 'modpath', title: f, text: f })),
          h('button', { class: 'btn ghost sm', 'data-act': 'fnfRemove', 'data-folder': f, disabled: ui.fnfBusy, text: 'Rimuovi' }))))
      : h('p', { class: 'empty', text: 'Nessuna mod collegata. Scegli la cartella di una mod fatta con Codename Engine (quella con il suo .exe).' }),
    h('button', { class: 'btn primary lg block', 'data-act': 'fnfAdd', disabled: ui.fnfBusy, text: ui.fnfBusy ? 'Collego…' : 'Aggiungi una mod' }),
    ui.fnfErr && h('p', { class: 'err', role: 'alert', text: ui.fnfErr }),
    h('p', { class: 'hint', text: 'Relay aggiunge un piccolo addon nella cartella della mod. Rimuovendola dalla lista viene tolto.' }),
  ];
}
function connSummary() {
  const on = [(ui.funkinFolders || []).length && 'Friday Night Funkin’', (ui.fnfFolders || []).length && 'Codename Engine', (ui.psychFolders || []).length && 'Psych Engine', (ui.nmvFolders || []).length && 'Nightmare Vision', (ui.kadeFolders || []).length && 'Kade Engine', (ui.gdFolders || []).length && 'Geometry Dash'].filter(Boolean);
  if (!on.length) return 'Collega Friday Night Funkin’, le sue mod e Geometry Dash';
  return (on.length > 1 ? on.slice(0, -1).join(', ') + ' e ' + on[on.length - 1] : on[0]) + (on.length === 1 ? ' collegato' : ' collegati');
}
function updateSaveMsg() { const e = $('savemsg'); if (e) e.textContent = ui.setDirty ? 'Salvataggio…' : ui.saveMsg; }

async function openSettings() {
  if (!ui.set) { try { ui.set = await invoke('get_settings'); } catch (e) { ui.notice = errMsg(e); } }
  if (!ui.fnfFolders) { try { ui.fnfFolders = await invoke('fnf_folders'); } catch (_) { ui.fnfFolders = []; } }
  if (!ui.funkinFolders) { try { ui.funkinFolders = await invoke('funkin_folders'); } catch (_) { ui.funkinFolders = []; } }
  if (!ui.psychFolders) { try { ui.psychFolders = await invoke('psych_folders'); } catch (_) { ui.psychFolders = []; } }
  if (!ui.nmvFolders) { try { ui.nmvFolders = await invoke('nmv_folders'); } catch (_) { ui.nmvFolders = []; } }
  if (!ui.kadeFolders) { try { ui.kadeFolders = await invoke('kade_folders'); } catch (_) { ui.kadeFolders = []; } }
  if (!ui.gdFolders) { try { ui.gdFolders = await invoke('gd_folders'); } catch (_) { ui.gdFolders = []; } }
  transition(() => { ui.view = 'settings'; ui.setPage = 'main'; renderNow(snap); setRev(); }, 'settings');
}
async function closeSettings() {
  await flushSave();
  transition(() => { ui.view = 'main'; ui.setPage = 'main'; renderNow(snap); }, 'back');
}
async function refreshWindows() {
  ui.winBusy = true; ui.winErr = null;
  if (snap && snap.match) renderMatch();
  try { ui.windows = await invoke('list_windows'); } catch (e) { ui.winErr = errMsg(e); ui.windows = ui.windows || []; }
  ui.winBusy = false;
  if (snap && snap.match) renderMatch();
}
async function refreshSteam() {
  ui.steamChecked = true;
  try { ui.steam = await invoke('detect_steam_game'); } catch (_) { ui.steam = null; }
  if (snap && snap.match) renderMatch();
}
async function searchGames() {
  const query = ui.gameQuery.trim();
  if (query.length < 2) { ui.gameResults = []; ui.gameBusy = false; if (snap && snap.match) renderMatch(); return; }
  ui.gameBusy = true;
  if (snap && snap.match) renderMatch();
  try { ui.gameResults = await invoke('search_steam_games', { query }); } catch (e) { ui.notice = errMsg(e); ui.gameResults = []; }
  ui.gameBusy = false;
  if (snap && snap.match) renderMatch();
}
function scheduleSave() {
  ui.setDirty = true; ui.saveMsg = ''; updateSaveMsg();
  clearTimeout(ui.saveTimer);
  ui.saveTimer = setTimeout(flushSave, 700);
}
async function flushSave() {
  clearTimeout(ui.saveTimer);
  if (!ui.setDirty || !ui.set) return;
  ui.setDirty = false;
  try {
    const saved = await invoke('save_settings', { settings: ui.set });
    if (!ui.setDirty && saved) ui.set = saved;
    ui.saveMsg = 'Salvato';
  } catch (e) { ui.saveMsg = ''; ui.notice = errMsg(e); }
  if (ui.view === 'settings') { setRev(); } renderBanner();
}

function setPreset(id) {
  const p = PRESETS.find((x) => x.id === id);
  if (!p) return;
  ui.set.preset = p.id; ui.set.fps = p.fps; ui.set.bitrate_kbps = p.bitrate_kbps;
}
function presetLine(s) {
  return s.fps + ' fps · ' + (s.bitrate_kbps / 1000).toFixed(s.bitrate_kbps % 1000 ? 1 : 0) + ' Mbit/s' + (s.preset === 'custom' ? ' · personalizzato' : '');
}
function updateChips() {
  document.querySelectorAll('[data-act=preset]').forEach((b) => b.setAttribute('aria-pressed', String(b.dataset.id === ui.set.preset)));
  const c = $('custom-chip'); if (c) c.textContent = presetLine(ui.set);
}
function updateSpeedProgress() {
  const b = $('sp-bar'), t = $('sp-txt');
  if (b) b.style.width = Math.round(ui.speed.progress * 100) + '%';
  if (t) t.textContent = 'Test in corso… ' + Math.round(ui.speed.progress * 100) + '% · ' + ui.speed.mbps.toFixed(1) + ' Mbit/s';
}

function field(label, control) { return h('label', { class: 'field' }, h('span', { text: label }), control); }
function sw(f, checked, text) {
  return h('label', { class: 'switch' }, h('input', { type: 'checkbox', 'data-f': f, 'data-k': f, checked }), h('span', { class: 'track' }), h('span', { text }));
}

function buildSettings() {
  const s = ui.set;
  if (!s) return [h('p', { class: 'muted', text: 'Impostazioni non disponibili.' })];
  const sp = ui.speed, rec = sp.result && sp.result.presets.find((p) => p.id === sp.result.recommended);
  const nofit = sp.result ? sp.result.presets.filter((p) => !p.fits) : [];
  const autoLimit = s.limit_kbps == null;
  const who = snap && snap.auth && snap.auth.logged_in ? (snap.auth.name || snap.auth.user || '') : '';

  return [
    who && h('div', { class: 'sec' },
      h('div', { class: 'label', text: 'Account' }),
      h('div', { class: 'acct' },
        h('span', { class: 'avatar', 'aria-hidden': 'true', text: who.trim().charAt(0).toUpperCase() }),
        h('span', { class: 'grow', text: who }),
        h('button', { class: 'btn ghost sm', 'data-act': 'logout', text: 'Esci' }))),

    h('div', { class: 'sec' },
      h('div', { class: 'label', text: 'Qualità' }),
      h('div', { class: 'seg', role: 'group', 'aria-label': 'Qualità' },
        PRESETS.map((p) => h('button', { class: nofit.some((n) => n.id === p.id) ? 'nofit' : '', 'data-act': 'preset', 'data-id': p.id, 'aria-pressed': String(s.preset === p.id), title: p.fps + ' fps · ' + p.bitrate_kbps + ' kbit/s', text: p.label }))),
      h('p', { id: 'custom-chip', class: 'hint', text: presetLine(s) }),
      h('div', { class: 'row', style: 'margin-top:10px' },
        h('button', { class: 'btn ghost sm', 'data-act': 'speedtest', disabled: sp.running, text: sp.running ? 'Test in corso…' : 'Test velocità' }),
        !sp.running && !sp.result && s.last_speedtest && h('span', { class: 'small muted', text: 'Ultimo test: ~' + Math.round(s.last_speedtest.upload_mbps) + ' Mbit/s' })),
      sp.running && h('div', null, h('div', { class: 'bar' }, h('i', { id: 'sp-bar' })), h('div', { id: 'sp-txt', class: 'small muted', 'aria-live': 'polite' })),
      sp.error && h('p', { class: 'err', role: 'alert', style: 'margin-top:8px', text: sp.error }),
      sp.result && !sp.running && h('div', { class: 'result stack' },
        h('div', null, 'Upload ~' + Math.round(sp.result.upload_mbps) + ' Mbit/s · consigliata ', h('strong', { text: rec ? rec.label : sp.result.recommended })),
        h('button', { class: 'btn primary sm', 'data-act': 'use-rec', text: 'Usa la consigliata' }),
        nofit.length ? h('div', { class: 'small muted', text: nofit.map((p) => p.label).join(', ') + ': troppo pesante per la tua connessione' }) : null)),

    h('div', { class: 'sec' },
      h('div', { class: 'label', text: 'Sul gioco' }),
      sw('overlay', s.overlay !== false, 'Pallino e tempo in alto a destra')),

    h('div', { class: 'sec' },
      h('details', { class: 'adv', 'data-k': 'adv', open: ui.advOpen },
        h('summary', { class: 'label', style: 'margin:24px 0 8px' }, ico('chevron', 14), 'Avanzate'),
        h('div', { class: 'stack' },
          h('div', { class: 'two' },
            field('FPS', h('input', { type: 'number', min: '1', max: '240', 'data-f': 'fps', 'data-k': 'fps', value: String(s.fps) })),
            field('Bitrate (kbit/s)', h('input', { type: 'number', min: '100', max: '100000', 'data-f': 'bitrate_kbps', 'data-k': 'bitrate', value: String(s.bitrate_kbps) }))),
          h('div', null,
            h('span', { class: 'small muted', text: 'Limite di upload (kbit/s)' }),
            h('div', { class: 'row', style: 'margin-top:4px' },
              h('label', { class: 'switch' }, h('input', { type: 'checkbox', 'data-f': 'limit_auto', 'data-k': 'limit_auto', checked: autoLimit }), h('span', { class: 'track' }), h('span', { text: 'Auto' })),
              h('input', { type: 'number', min: '100', class: 'grow', 'data-f': 'limit_kbps', 'data-k': 'limit', value: autoLimit ? '' : String(s.limit_kbps), disabled: autoLimit, placeholder: 'Auto', 'aria-label': 'Limite upload in kbit/s' })),
            autoLimit && h('p', { class: 'hint', text: 'Auto = metà dell’upload misurato.' })),
          field('Encoder', h('select', { 'data-f': 'encoder', 'data-k': 'encoder' }, ENCODERS.map(([v, t]) => h('option', { value: v, text: t, selected: s.encoder === v })))),
          captureInfo()))),
    h('div', { class: 'sec' },
      h('div', { class: 'label', text: 'Integrazioni' }),
      h('button', { class: 'navrow', 'data-act': 'openConnectors' },
        h('span', { class: 'navrow-ico' }, h('img', { src: CNE_LOGO, alt: '' })),
        h('span', { class: 'txt' },
          h('span', { class: 't1', text: 'Connettori' }),
          h('span', { class: 't2', text: connSummary() })),
        ico('chevron', 16))),

    h('div', { class: 'sec' },
      h('div', { class: 'label', text: 'Informazioni' }),
      h('div', { class: 'row' },
        h('span', { class: 'grow', text: 'Relay ' + ((snap && snap.version) || '') }),
        h('button', { class: 'btn ghost sm', 'data-act': 'checkUpdate', disabled: ui.upd === 'checking', text: ui.upd === 'checking' ? 'Controllo…' : 'Controlla aggiornamenti' })),
      ui.updMsg && h('p', { class: ui.updMsg.bad ? 'err' : 'hint', role: 'status', style: 'margin:6px 0 0', text: ui.updMsg.text }),
      snap && snap.update && snap.update.ready && !snap.match && h('div', { class: 'row', style: 'margin-top:8px' },
        h('button', { class: 'btn primary sm', 'data-act': 'applyUpdate', text: 'Riavvia e installa ' + snap.update.version })),
      h('div', { class: 'row', style: 'margin-top:8px' },
        h('button', { class: 'btn ghost sm', 'data-act': 'openLogs', text: 'Apri la cartella dei log' }),
        h('span', { class: 'small muted', text: 'Se qualcosa si ferma, il file relay.log dice perché' }))),
  ];
}

function onField(e, final) {
  const t = e.target, f = t.dataset && t.dataset.f;
  if (!f) return;
  if (f === 'newName') { ui.newName = t.value; return; }
  if (f === 'renameDraft') { ui.renameDraft = t.value; return; }
  if (f === 'joinLink') { ui.joinLink = t.value; return; }
  if (f === 'game-query') {
    ui.gameQuery = t.value;
    clearTimeout(ui.gameSearchTimer);
    ui.gameSearchTimer = setTimeout(searchGames, 350);
    return;
  }
  const s = ui.set;
  if (!s) return;
  if (f === 'window') {
    if (t.value === '') s.window = null;
    else if (t.value !== 'cur') { const x = ui.windows[+t.value]; s.window = { exe: x.exe, title: x.title }; }

    invoke('set_window', { window: s.window }).catch((err) => { ui.notice = errMsg(err); renderBanner(); });
    if (snap && snap.match) renderMatch();
    return;
  }
  if (f === 'limit_auto') {
    const half = s.last_speedtest ? Math.round(s.last_speedtest.upload_mbps * 500) : 5000;
    s.limit_kbps = t.checked ? null : half;
    scheduleSave(); setRev(); return;
  }
  if (f === 'overlay') { s.overlay = t.checked; scheduleSave(); return; }
  if (f === 'fnf_autorecord' || f === 'fnf_mic') { s[f] = t.checked; scheduleSave(); setRev(); return; }
  if (f === 'audio_game' || f === 'audio_mic' || f === 'audio_mic_gain') {
    if (f === 'audio_mic_gain') {
      s.audio_mic_gain = (+t.value) / 100;
      const v = document.getElementById('mic-gain-val'); if (v) v.textContent = Math.round(+t.value) + '%';

      if (!final) return;
    } else s[f] = t.checked;

    invoke('set_audio', { game: !!s.audio_game, mic: !!s.audio_mic, micGain: s.audio_mic_gain || 3 }).catch((err) => { ui.notice = errMsg(err); renderBanner(); });
    if (snap && snap.match) renderMatch();
    return;
  }
  if (f === 'fps' || f === 'bitrate_kbps' || f === 'limit_kbps') {
    const v = parseInt(t.value, 10);
    const [lo, hi] = f === 'fps' ? [1, 240] : [100, 100000];
    if (!(v > 0)) { if (final && f !== 'limit_kbps') t.value = String(s[f]); return; }
    const c = final ? Math.min(hi, Math.max(lo, v)) : v;
    if (final) t.value = String(c);
    if (final || (v >= lo && v <= hi)) {
      s[f] = c;
      if (f !== 'limit_kbps') { s.preset = 'custom'; updateChips(); }
      scheduleSave();
    }
    return;
  }
  s[f] = t.value;
  if (final || t.tagName === 'INPUT') scheduleSave();
}
document.addEventListener('input', (e) => onField(e, false));
document.addEventListener('change', (e) => onField(e, true));

document.addEventListener('toggle', (e) => { if (e.target.tagName === 'DETAILS') ui.advOpen = e.target.open; }, true);

async function run(name, fn) {
  ui.busy = name; ui.notice = null;
  render(snap);
  try { await fn(); } catch (e) { ui.notice = errMsg(e); }
  ui.busy = null;
  try { render(await invoke('get_state')); } catch (_) { render(snap); }
}

async function copyInvite() {
  const url = snap && snap.match && snap.match.invite_url;
  if (!url) return;
  let ok = false;
  try { await navigator.clipboard.writeText(url); ok = true; } catch (_) {}
  if (!ok) {
    const i = $('invite-input');
    if (i) { i.focus(); i.select(); try { ok = document.execCommand('copy'); } catch (_) {} }
  }
  if (!ok) { ui.notice = 'Copia non riuscita: seleziona il link e premi Ctrl+C.'; renderBanner(); return; }
  ui.copied = true; renderMatch();
  clearTimeout(copyInvite.t);
  copyInvite.t = setTimeout(() => { ui.copied = false; if (snap && snap.match) renderMatch(); }, 2000);
}

const ACTIONS = {
  login: () => {
    ui.loginErr = null;
    return run('login', async () => {
      try { await invoke('login'); } catch (e) { ui.loginErr = errMsg(e); }
    });
  },
  logout: async () => { await flushSave(); ui.view = 'main'; await run('logout', () => invoke('logout')); },
  applyUpdate: () => run('applyUpdate', () => invoke('apply_update')),
  checkUpdate: async () => {
    ui.upd = 'checking'; ui.updMsg = null; setRev();
    try {
      const r = await invoke('check_update');
      const cur = (snap && snap.version) || '';
      ui.updMsg = r.state === 'ready' ? { text: 'Aggiornamento ' + r.version + ' pronto: si installa da solo quando nessuno usa l’app, oppure subito con il pulsante qui sotto.' }
        : r.state === 'busy' ? { text: 'Esci dalla partita per controllare: durante una registrazione l’app non fa nulla in più.' }
        : r.state === 'error' ? { text: 'Controllo non riuscito: ' + r.error, bad: true }
        : { text: 'Sei aggiornato: la versione ' + cur + ' è l’ultima.' };
    } catch (e) { ui.updMsg = { text: errMsg(e), bad: true }; }
    ui.upd = null;
    try { render(await invoke('get_state')); } catch (_) {  }
    setRev();
  },
  openLogs: () => invoke('open_logs').catch((e) => { ui.notice = errMsg(e); renderBanner(); }),
  retryFfmpeg: () => invoke('retry_ffmpeg').catch((e) => { ui.notice = errMsg(e); renderBanner(); }),
  retryCapture: () => invoke('retry_capture').catch((e) => { ui.notice = errMsg(e); renderBanner(); }),
  skipCapture: () => invoke('skip_capture').catch((e) => { ui.notice = errMsg(e); renderBanner(); }),
  gear: () => openSettings(),
  back: () => (['cne', 'funkin', 'psych', 'nmv', 'kade', 'gd'].includes(ui.setPage) ? goSetPage('connectors', 'back') : ui.setPage === 'connectors' ? goSetPage('main', 'back') : closeSettings()),
  openConnectors: () => goSetPage('connectors', 'settings'),
  openCne: () => goSetPage('cne', 'settings'),
  openFunkin: () => goSetPage('funkin', 'settings'),
  openPsych: () => goSetPage('psych', 'settings'),
  openNmv: () => goSetPage('nmv', 'settings'),
  openKade: () => goSetPage('kade', 'settings'),
  openGd: () => goSetPage('gd', 'settings'),
  dismiss: () => { if (ui.notice) ui.notice = null; else ui.dismissedErr = snap.error; renderBanner(); },
  acceptInvite: () => run('acceptInvite', () => invoke('accept_invite')),
  declineInvite: () => run('declineInvite', () => invoke('decline_invite')),
  create: () => run('create', async () => { const name = ui.newName.trim(); await invoke('create_match', { name: name || null }); ui.newName = ''; }),
  renameStart: () => { ui.renaming = true; ui.renameDraft = (snap.match && snap.match.name) || ''; render(snap); const i = document.getElementById('rename-input'); if (i) { i.focus(); i.select(); } },
  renameCancel: () => { ui.renaming = false; render(snap); },
  showJoin: () => { ui.showJoin = true; render(snap); const i = document.getElementById('join-input'); if (i) i.focus(); },
  open: (b) => run('open', () => invoke('open_match', { id: b.dataset.id })),
  toggle: (b) => {
    const id = b.dataset.id;
    if (ui.open.has(id)) ui.open.delete(id); else ui.open.add(id);
    if (snap && snap.match) renderMatch();
  },
  copy: () => copyInvite(),
  start: () => run('start', () => invoke('host_start', { force: false, game: ui.game })),
  force: () => {
    if (!ui.forceArmed) {
      ui.forceArmed = true; renderMatch();
      clearTimeout(ACTIONS.force.t);
      ACTIONS.force.t = setTimeout(() => { ui.forceArmed = false; if (snap && snap.match) renderMatch(); }, 4000);
      const nb = document.querySelector('[data-act=force]'); if (nb) nb.focus();
      return;
    }
    ui.forceArmed = false;
    return run('start', () => invoke('host_start', { force: true, game: ui.game }));
  },
  stop: () => run('stop', () => invoke('host_stop')),
  leave: () => run('leave', () => invoke('leave_match')),
  newmatch: () => run('leave', () => invoke('leave_match')),
  replay: () => invoke('open_url', { url: snap.match.replay_url }).catch((e) => { ui.notice = errMsg(e); renderBanner(); }),
  'refresh-windows': () => refreshWindows(),
  pickGame: (b) => {
    ui.game = b.dataset.i === 'steam' ? ui.steam : ui.gameResults[+b.dataset.i] || null;
    if (snap && snap.match) renderMatch();
  },
  clearGame: () => {
    ui.game = null; ui.gameQuery = ''; ui.gameResults = [];
    if (snap && snap.match) renderMatch();
    const i = document.querySelector('[data-k="game-query"]'); if (i) i.focus();
  },
  preset: (b) => { setPreset(b.dataset.id); scheduleSave(); setRev(); },
  speedtest: async () => {
    ui.speed = { running: true, progress: 0, mbps: 0, result: null, error: null };
    setRev(); updateSpeedProgress();
    try { ui.speed.result = await invoke('run_speedtest'); } catch (e) { ui.speed.error = errMsg(e); }
    ui.speed.running = false;
    if (ui.speed.result && ui.set) ui.set.last_speedtest = { upload_mbps: ui.speed.result.upload_mbps, at: new Date().toISOString() };
    setRev();
  },
  'use-rec': () => {
    const r = ui.speed.result;
    if (!r || !ui.set) return;
    setPreset(r.recommended);
    ui.set.limit_kbps = r.limit_kbps;
    scheduleSave(); setRev();
  },
  fnfAdd: async () => {
    ui.fnfErr = null;
    let folder;
    try { folder = await invoke('fnf_pick_folder'); } catch (e) { ui.fnfErr = errMsg(e); setRev(); return; }
    if (!folder) return;
    ui.fnfBusy = true; setRev();
    try { ui.fnfFolders = await invoke('fnf_add_folder', { folder }); } catch (e) { ui.fnfErr = errMsg(e); }
    ui.fnfBusy = false; setRev();
  },
  funkinAdd: async () => {
    ui.fnfErr = null;
    let folder;
    try { folder = await invoke('fnf_pick_folder', { title: 'Cartella di Friday Night Funkin’' }); } catch (e) { ui.fnfErr = errMsg(e); setRev(); return; }
    if (!folder) return;
    ui.fnfBusy = true; setRev();
    try { ui.funkinFolders = await invoke('funkin_add_folder', { folder }); } catch (e) { ui.fnfErr = errMsg(e); }
    ui.fnfBusy = false; setRev();
  },
  psychAdd: async () => {
    ui.fnfErr = null;
    let folder;
    try { folder = await invoke('fnf_pick_folder', { title: 'Cartella della mod Psych Engine' }); } catch (e) { ui.fnfErr = errMsg(e); setRev(); return; }
    if (!folder) return;
    ui.fnfBusy = true; setRev();
    try { ui.psychFolders = await invoke('psych_add_folder', { folder }); } catch (e) { ui.fnfErr = errMsg(e); }
    ui.fnfBusy = false; setRev();
  },
  scanMods: async () => {
    ui.scanErr = null; ui.scanBusy = true; setRev();
    try {
      ui.scanResult = await invoke('scan_mods');
      const [a, b, c, d, e, f] = await Promise.all(['fnf_folders', 'funkin_folders', 'psych_folders', 'nmv_folders', 'gd_folders', 'kade_folders'].map((k) => invoke(k).catch(() => [])));
      ui.fnfFolders = a; ui.funkinFolders = b; ui.psychFolders = c; ui.nmvFolders = d; ui.gdFolders = e; ui.kadeFolders = f;
    } catch (e) { ui.scanErr = errMsg(e); }
    ui.scanBusy = false; setRev();
  },
  gdAdd: async () => {
    ui.fnfErr = null; ui.fnfBusy = true; setRev();
    try {
      const folder = await invoke('gd_find');
      if (!folder) ui.fnfErr = 'Non trovo Geometry Dash nelle librerie di Steam: scegli la cartella a mano.';
      else ui.gdFolders = await invoke('gd_add_folder', { folder });
    } catch (e) { ui.fnfErr = errMsg(e); }
    ui.fnfBusy = false; setRev();
  },
  gdPick: async () => {
    ui.fnfErr = null;
    let folder;
    try { folder = await invoke('fnf_pick_folder', { title: 'Cartella di Geometry Dash' }); } catch (e) { ui.fnfErr = errMsg(e); setRev(); return; }
    if (!folder) return;
    ui.fnfBusy = true; setRev();
    try { ui.gdFolders = await invoke('gd_add_folder', { folder }); } catch (e) { ui.fnfErr = errMsg(e); }
    ui.fnfBusy = false; setRev();
  },
  gdRemove: async (b) => {
    ui.fnfErr = null;
    try { ui.gdFolders = await invoke('gd_remove_folder', { folder: b.dataset.folder }); } catch (e) { ui.fnfErr = errMsg(e); }
    setRev();
  },
  nmvAdd: async () => {
    ui.fnfErr = null;
    let folder;
    try { folder = await invoke('fnf_pick_folder', { title: 'Cartella della mod Nightmare Vision' }); } catch (e) { ui.fnfErr = errMsg(e); setRev(); return; }
    if (!folder) return;
    ui.fnfBusy = true; setRev();
    try { ui.nmvFolders = await invoke('nmv_add_folder', { folder }); } catch (e) { ui.fnfErr = errMsg(e); }
    ui.fnfBusy = false; setRev();
  },
  kadeAdd: async () => {
    ui.fnfErr = null;
    let folder;
    try { folder = await invoke('fnf_pick_folder', { title: 'Cartella della mod Kade Engine' }); } catch (e) { ui.fnfErr = errMsg(e); setRev(); return; }
    if (!folder) return;
    ui.fnfBusy = true; setRev();
    try { ui.kadeFolders = await invoke('kade_add_folder', { folder }); } catch (e) { ui.fnfErr = errMsg(e); }
    ui.fnfBusy = false; setRev();
  },
  kadeRemove: async (b) => {
    ui.fnfErr = null;
    try { ui.kadeFolders = await invoke('kade_remove_folder', { folder: b.dataset.folder }); } catch (e) { ui.fnfErr = errMsg(e); }
    setRev();
  },
  nmvRemove: async (b) => {
    ui.fnfErr = null;
    try { ui.nmvFolders = await invoke('nmv_remove_folder', { folder: b.dataset.folder }); } catch (e) { ui.fnfErr = errMsg(e); }
    setRev();
  },
  psychRemove: async (b) => {
    ui.fnfErr = null;
    try { ui.psychFolders = await invoke('psych_remove_folder', { folder: b.dataset.folder }); } catch (e) { ui.fnfErr = errMsg(e); }
    setRev();
  },
  funkinRemove: async (b) => {
    ui.fnfErr = null;
    try { ui.funkinFolders = await invoke('funkin_remove_folder', { folder: b.dataset.folder }); } catch (e) { ui.fnfErr = errMsg(e); }
    setRev();
  },
  fnfRemove: async (b) => {
    ui.fnfErr = null;
    try { ui.fnfFolders = await invoke('fnf_remove_folder', { folder: b.dataset.folder }); } catch (e) { ui.fnfErr = errMsg(e); }
    setRev();
  },
};

document.addEventListener('click', (e) => {
  const b = e.target.closest('[data-act]');
  if (!b || b.disabled) return;
  const fn = ACTIONS[b.dataset.act];
  if (fn) fn(b);
});
document.addEventListener('submit', (e) => {
  e.preventDefault();
  if (e.target.dataset.form === 'rename') {
    const name = ui.renameDraft.trim();
    run('rename', async () => { await invoke('rename_match', { name }); ui.renaming = false; });
    return;
  }
  if (e.target.dataset.form !== 'join') return;
  const link = ui.joinLink.trim();
  if (!link) { ui.joinErr = 'Incolla il link di invito.'; renderHome(); return; }
  ui.joinErr = null;
  run('join', async () => {
    try { await invoke('join_match', { link }); ui.joinLink = ''; } catch (err) { ui.joinErr = errMsg(err); }
  });
});

boot();
