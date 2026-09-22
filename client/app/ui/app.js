'use strict';

const PRESETS = [
  { id: 'low', label: 'Leggera', fps: 30, bitrate_kbps: 2500 },
  { id: 'medium', label: 'Standard', fps: 60, bitrate_kbps: 4000 },
  { id: 'high', label: 'Alta', fps: 60, bitrate_kbps: 6000 },
  { id: 'max', label: 'Massima', fps: 60, bitrate_kbps: 9000 },
];
const ENCODERS = [
  ['auto', 'Automatico'], ['nvenc', 'NVIDIA (nvenc)'], ['amf', 'AMD (amf)'],
  ['qsv', 'Intel (qsv)'], ['x264', 'Software (x264)'],
];
const STATUS_MATCH = { waiting: 'In attesa', recording: 'In registrazione', done: 'Completata' };

let T = null;
let snap = null;

const ui = {
  view: 'main', prevView: null, busy: null, notice: null, dismissedErr: null,
  loginErr: null, joinErr: null, joinLink: '', newName: '', renaming: false, renameDraft: '', showJoin: false,
  recents: null, recentsLoading: false,
  copied: false, forceArmed: false, open: new Set(),
  set: null, setDirty: false, saveMsg: '', saveTimer: null,
  windows: null, winBusy: false, winErr: null,
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

function currentView() {
  if (!snap) return 'loading';
  if (ui.view === 'settings') return 'settings';
  if (snap.capture && snap.capture.state !== 'ready') return 'install';
  if (!snap.auth || !snap.auth.logged_in) return 'login';
  return snap.match ? 'match' : 'home';
}

function render(s) {
  snap = s;
  if (!s) return;
  const m = s.match;
  if (m && m.can_start) ui.forceArmed = false;
  const view = currentView();
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
      h('h1', { text: 'Relay' }),
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
  return { nvenc: 'scheda NVIDIA', amf: 'scheda AMD', qsv: 'grafica Intel', x264: 'processore' }[id] || id;
}

function renderInstall() {
  region('v-install', JSON.stringify(snap.capture), () => {
    const c = snap.capture;
    const groups = c.stage === 'obs' ? c.groups || [] : [];
    const compat = c.compat;
    return [
      h('div', { class: 'hero-login' },
        h('div', { class: 'bigdot', 'aria-hidden': 'true' }),
        h('h1', { text: 'Relay' }),
        h('p', { text: installStageLabel(c) }),
        c.stage !== 'obs' && c.state !== 'error' && h('div', { class: 'bar' }, h('i', { style: 'width:' + Math.round((c.percent || 0) * 100) + '%' })),
        groups.map((g) => h('div', { class: 'instgroup' },
          h('div', { class: 'row' }, h('span', { class: 'small', text: g.label }), h('span', { class: 'small muted', text: Math.round(g.percent) + '%' })),
          h('div', { class: 'bar' }, h('i', { style: 'width:' + Math.round(g.percent) + '%' })))),
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
}

function renderHome() {
  const inv = snap && snap.invite_request;
  region('v-home', JSON.stringify([ui.recents, ui.busy, ui.joinErr, inv, ui.showJoin]), () => [
    ...(inv ? [h('div', { class: 'invitecard', role: 'alert' },
      h('div', { class: 't1', text: 'Invito a una partita' }),
      h('div', { class: 'small muted', text: 'Qualcuno ti ha invitato con un link. Entri solo se sei stato invitato da una persona che conosci.' }),
      h('div', { class: 'row' },
        h('button', { class: 'btn primary', 'data-act': 'acceptInvite', disabled: !!ui.busy, text: ui.busy === 'acceptInvite' ? 'Entro…' : 'Entra' }),
        h('button', { class: 'btn ghost', 'data-act': 'declineInvite', disabled: !!ui.busy, text: 'Ignora' })))] : []),
    h('div', { class: 'card hero-home' },
      h('input', { type: 'text', 'data-f': 'newName', 'data-k': 'newName', value: ui.newName, maxlength: '60', placeholder: 'Nome (facoltativo)', 'aria-label': 'Nome della partita', autocomplete: 'off', spellcheck: 'false' }),
      h('button', { class: 'btn primary lg block', 'data-act': 'create', disabled: !!ui.busy, text: ui.busy === 'create' ? 'Creo…' : 'Crea partita' }),),

    ui.showJoin || ui.joinErr
      ? h('form', { 'data-form': 'join', style: 'margin-top:14px' },
          h('div', { class: 'pillfield' },
            h('input', { type: 'text', id: 'join-input', 'data-f': 'joinLink', 'data-k': 'joinLink', value: ui.joinLink, placeholder: 'Incolla il link di invito', 'aria-label': 'Link di invito', autocomplete: 'off', spellcheck: 'false' }),
            h('button', { type: 'submit', class: 'iconbtn', disabled: !!ui.busy, 'aria-label': 'Entra' }, ico('arrow', 16))),
          ui.joinErr && h('p', { class: 'err', style: 'margin:6px 2px 0', role: 'alert', text: ui.joinErr }))
      : h('div', { class: 'center', style: 'margin-top:10px' }, h('button', { class: 'linkbtn', 'data-act': 'showJoin', text: 'Entra con un link' })),

    h('div', { class: 'label', text: 'Recenti' }),
    ui.recents == null ? h('p', { class: 'empty', text: 'Caricamento…' })
      : !ui.recents.length ? h('p', { class: 'empty', text: 'Nessuna partita ancora.' })
      : h('ul', { class: 'rows' }, ui.recents.map((r) => h('li', null,
          h('button', { class: 'rowbtn', 'data-act': 'open', 'data-id': r.id, disabled: !!ui.busy },
            h('span', { class: 'dot ' + (r.status === 'recording' ? 'rec' : r.status === 'done' ? 'ok' : '') }),
            h('span', { class: 'txt' },
              h('div', { class: 't1', text: r.name || (r.players && r.players.length ? r.players.join(', ') : 'Nessun giocatore') }),
              h('div', { class: 't2', text: (STATUS_MATCH[r.status] || r.status) + (r.host ? ' · host' : '') + ' · ' + new Date(r.created_at * 1000).toLocaleString('it-IT', { day: '2-digit', month: '2-digit', hour: '2-digit', minute: '2-digit' }) })))))),
  ]);
}

function dotClass(p) {
  if (!p.connected || p.state === 'error') return 'bad';
  if (p.state === 'recording' && p.ready) return 'rec';
  if (p.ready) return 'ok';
  if (p.state === 'idle' && p.window_found == null && !p.issue) return '';
  return 'warn';
}
function statusInfo(p) {
  if (!p.connected) return ['offline', 'bad'];
  if (p.state === 'error') return ['errore', 'bad'];
  if (p.ready) return p.state === 'recording' ? ['registra', 'rec'] : p.state === 'uploading' ? ['carica', ''] : ['pronto', 'ok'];
  return ['non pronto', 'warn'];
}

function detailParts(p) {
  const out = [];
  if (!p.connected) return [{ t: 'non connesso al server', c: 'bad' }];
  if (p.issue) out.push({ t: p.issue, c: p.state === 'error' ? 'bad' : 'warn' });
  if (p.audio_issue) out.push({ t: p.audio_issue, c: 'warn' });
  else if (p.audio) out.push({ t: 'audio: ' + p.audio });
  if (p.window_found === false && !(p.issue || '').includes('finestra')) out.push({ t: 'finestra non trovata', c: 'bad' });
  else if (p.window_found === true) out.push({ t: 'finestra ok' });
  out.push({ t: p.rtt_ms != null ? 'ping ' + p.rtt_ms + ' ms' : 'ping –' });
  out.push({ t: 'coda ' + (p.backlog || 0) });
  if (p.upload_kbps > 0) out.push({ t: p.upload_kbps + ' kbit/s' });
  return out;
}
function detailsLine(parts) {
  const el = h('div', { class: 'details' });
  parts.forEach((x, i) => {
    if (i) el.append(' · ');
    el.append(x.c ? h('span', { class: x.c, text: x.t }) : x.t);
  });
  return el;
}
const needsAttention = (p) => !p.connected || p.state === 'error' || (!p.ready && p.state !== 'idle') || !!p.issue || !!p.audio_issue;

function headBlock(m, isHost) {
  const total = m.players.length, ready = m.players.filter((p) => p.ready).length;
  switch (m.phase) {
    case 'recording':
      return [h('div', { class: 'statuslabel rec' }, h('span', { class: 'rdot' }), 'In registrazione'),
        h('div', { id: 'elapsed', class: 'timer' })];
    case 'stopping': case 'uploading': {
      const q = m.me && m.me.backlog;
      return [h('div', { class: 'spinner', 'aria-hidden': 'true' }),
        h('div', { class: 'headline' }, 'Carico i pezzi', h('small', { text: q ? q + ' in coda · non chiudere l’app' : 'Non chiudere l’app' }))];
    }
    case 'done':
      return [h('div', { class: 'tick' }, ico('check', 24)), h('div', { class: 'headline', text: 'Registrazione completa' })];
    case 'error':
      return [h('div', { class: 'statuslabel', text: 'Errore' }), h('div', { class: 'headline', text: 'Qualcosa non va' })];
    default:

      if (m.started_at_ms != null && (m.role === 'player' || m.role === 'host_player')) {
        return [h('div', { class: 'statuslabel rec' }, h('span', { class: 'rdot' }), 'Partita in corso'),
          h('div', { class: 'headline' }, 'Riprendi a registrare', h('small', { text: 'Scegli il gioco: riparte da solo' }))];
      }
      return [
        h('div', { class: 'headline' },
          isHost ? (total ? ready + ' su ' + total + ' pronti' : 'Nessun giocatore') : 'In attesa dell’host',
          isHost && !total ? h('small', { text: 'Condividi il link di invito' }) : null)];
  }
}

function titleBlock(m, isHost) {
  if (ui.renaming && isHost) {
    return [h('form', { 'data-form': 'rename', class: 'renamebar' },
      h('input', { type: 'text', id: 'rename-input', 'data-f': 'renameDraft', maxlength: '60', value: ui.renameDraft, placeholder: 'Nome della partita', 'aria-label': 'Nome della partita', autocomplete: 'off', spellcheck: 'false' }),
      h('button', { type: 'submit', class: 'btn primary sm', disabled: !!ui.busy, text: 'Salva' }),
      h('button', { type: 'button', class: 'btn ghost sm', 'data-act': 'renameCancel', text: 'Annulla' }))];
  }
  if (!m.name && !isHost) return [];
  return [h('div', { class: 'titlebar' },
    h('span', { class: 'mtitle' + (m.name ? '' : ' auto'), title: m.name || '', text: m.name || 'Partita senza nome' }),
    isHost && h('button', { class: 'iconbtn', 'data-act': 'renameStart', 'aria-label': 'Rinomina la partita', title: 'Rinomina la partita' }, ico('edit', 15)))];
}

function renderMatch() {
  const m = snap.match, me = snap.auth.user;
  const isHost = m.role === 'host' || m.role === 'host_player';
  region('m-title', JSON.stringify([m.name, isHost, ui.renaming, ui.busy === 'rename']), () => titleBlock(m, isHost));
  region('m-head', JSON.stringify([m.phase, isHost, m.players.map((p) => p.ready), m.me && m.me.backlog, m.started_at_ms != null]), () => headBlock(m, isHost));
  tickElapsed();

  renderWindowPicker(m);

  region('m-players', JSON.stringify([m.players, me, m.me, isHost, [...ui.open]]), () => {
    const items = m.players.map((p) => {
      const [label, cls] = statusInfo(p);
      const open = needsAttention(p) || ui.open.has(p.id);
      return h('li', null,
        h('button', { class: 'prow', 'data-act': 'toggle', 'data-id': p.id, 'data-k': 'p-' + p.id, 'aria-expanded': String(open) },
          h('div', { class: 'line' },
            h('span', { class: 'dot ' + dotClass(p), 'aria-hidden': 'true' }),
            h('span', { class: 'pname', text: p.name + (p.id === me ? ' (tu)' : '') }),
            (p.host || p.is_host || (isHost && p.id === me && m.role === 'host_player')) && h('span', { class: 'tag', text: 'host' }),
            h('span', { class: 'stlabel ' + cls, text: label })),
          p.window && h('div', { class: 'wline', title: p.window, text: p.window }),
          open && detailsLine(detailParts(p))));
    });
    if (!items.length) return [];
    return [h('ul', { class: 'players', 'aria-label': 'Giocatori' }, items)];
  });

  region('m-invite', JSON.stringify([isHost && m.phase === 'lobby' ? m.invite_url : null, ui.copied]), () => {
    if (!(isHost && m.phase === 'lobby' && m.invite_url)) return [];
    return [h('div', { class: 'invite' },
      h('button', { class: 'btn ghost sm block', 'data-act': 'copy', 'data-k': 'copy' }, ico(ui.copied ? 'check' : 'link', 15), ui.copied ? 'Link copiato' : 'Copia il link di invito'),
      h('input', { type: 'text', class: 'sr', id: 'invite-input', readOnly: true, value: m.invite_url, 'aria-label': 'Link di invito', tabindex: '-1' }))];
  });

  const canStart = !!m.can_start;
  region('m-actions', JSON.stringify([m.phase, m.role, m.started_at_ms != null, canStart, m.start_blockers, ui.forceArmed, ui.busy, m.replay_url]), () => {
    const out = [];
    const busy = !!ui.busy;
    const leave = (txt) => h('div', { class: 'center' }, h('button', { class: 'linkbtn', 'data-act': 'leave', disabled: busy, text: txt || 'Esci dalla partita' }));
    if (m.phase === 'lobby' && m.started_at_ms != null) {
      out.push(h('p', { class: 'dockhint', text: 'La registrazione riprende appena il gioco è aperto e scelto. Il tratto mancante resta scuro.' }));
      if (isHost && m.stopped_at_ms == null) out.push(h('button', { class: 'btn danger lg block', 'data-act': 'stop', disabled: busy, text: ui.busy === 'stop' ? 'Fermo…' : 'Ferma' }));
      out.push(leave());
    } else if (m.phase === 'lobby') {
      if (isHost) {
        if (!canStart && !m.players.length) out.push(h('p', { class: 'blockers', id: 'blk', text: (m.start_blockers || []).join(' · ') }));
        out.push(h('button', { class: 'btn primary lg block', 'data-act': 'start', disabled: !canStart || busy, 'aria-describedby': 'blk', text: ui.busy === 'start' ? 'Avvio…' : 'Avvia' }));
        out.push(h('div', { class: 'dockrow' },
          !canStart ? h('button', { class: 'linkbtn' + (ui.forceArmed ? ' armed' : ''), 'data-act': 'force', disabled: busy, text: ui.forceArmed ? 'Confermi? Avvia comunque' : 'Avvia comunque' }) : null,
          h('button', { class: 'linkbtn', 'data-act': 'leave', disabled: busy, text: 'Esci dalla partita' })));
      } else {
        out.push(h('p', { class: 'dockhint', text: 'Parte quando l’host avvia la registrazione.' }));
        out.push(leave());
      }
    } else if (m.phase === 'recording') {
      if (isHost) out.push(h('button', { class: 'btn danger lg block', 'data-act': 'stop', disabled: busy, text: ui.busy === 'stop' ? 'Fermo…' : 'Ferma' }));
      else out.push(h('p', { class: 'dockhint', text: 'Tieni aperto il gioco: al resto pensa Relay.' }));
    } else if (m.phase === 'done') {
      out.push(h('button', { class: 'btn primary lg block', 'data-act': 'replay', text: 'Apri replay' }));
      out.push(h('button', { class: 'btn ghost block', 'data-act': 'newmatch', disabled: busy, text: 'Nuova partita' }));
    } else if (m.phase === 'error') {
      out.push(h('p', { class: 'err center', text: snap.error || 'Si è verificato un errore.' }));
      out.push(leave('Esci'));
    }
    return out;
  });
}

function renderWindowPicker(m) {
  const plays = m.role === 'player' || m.role === 'host_player';
  const show = plays && m.phase === 'lobby' && !!ui.set;
  if (show && ui.windows == null && !ui.winBusy) refreshWindows();
  region('m-window', JSON.stringify([show, ui.windows, ui.set && ui.set.window, ui.winBusy, ui.winErr, ui.set && [ui.set.audio_game, ui.set.audio_mic, ui.set.audio_mic_gain]]), () => {
    if (!show) return [];
    const w = ui.set.window;
    const list = ui.windows || [];

    const both = w ? list.findIndex((x) => x.exe === w.exe && x.title === w.title) : -1;
    const idx = w ? (both >= 0 ? both : list.findIndex((x) => x.exe === w.exe)) : -1;
    const selVal = !w ? '' : idx >= 0 ? String(idx) : 'cur';
    const isMon = (x) => !!x && String(x.exe).startsWith('@monitor:');
    const monitor = isMon(w);
    const label = (x) => (isMon(x) ? x.title : x.title + ' — ' + x.exe);
    const opts = [h('option', { value: '', text: '— scegli cosa registrare —', selected: selVal === '' })];
    if (w && idx < 0) opts.push(h('option', { value: 'cur', text: label(w) + ' (non trovata)', selected: true }));
    list.forEach((x, i) => opts.push(h('option', { value: String(i), text: label(x), selected: selVal === String(i) })));
    return [h('div', { class: 'wsel' },
      h('div', { class: 'label', text: 'La tua finestra' }),
      h('div', { class: 'row' },
        h('select', { class: 'grow', 'data-f': 'window', 'data-k': 'window', 'aria-label': 'Finestra da registrare', title: w && !monitor ? 'Riconosciuta dal programma: ' + w.exe : '' }, opts),
        h('button', { class: 'iconbtn', 'data-act': 'refresh-windows', disabled: ui.winBusy, 'aria-label': 'Aggiorna elenco', title: 'Aggiorna elenco' }, ico('refresh', 16))),
      ui.winErr && h('p', { class: 'err', style: 'margin:6px 2px 0', text: ui.winErr }),
      monitor && h('p', { class: 'hint warn', text: 'Registra tutto lo schermo, notifiche comprese: chiudi ciò che non vuoi mostrare.' }),
      h('div', { class: 'label', text: 'Audio' }),
      h('div', { class: 'audiosw' },
        h('label', { class: 'switch' }, h('input', { type: 'checkbox', 'data-f': 'audio_game', 'data-k': 'audio_game', checked: !!ui.set.audio_game }), h('span', { class: 'track' }), h('span', { text: 'Audio del gioco' })),
        h('label', { class: 'switch' }, h('input', { type: 'checkbox', 'data-f': 'audio_mic', 'data-k': 'audio_mic', checked: !!ui.set.audio_mic }), h('span', { class: 'track' }), h('span', { text: 'Microfono' }))),
      ui.set.audio_mic && h('div', { class: 'micgain' },
        h('div', { class: 'row' },
          h('span', { class: 'small', text: 'Volume del microfono' }),
          h('span', { id: 'mic-gain-val', class: 'small muted', text: Math.round((ui.set.audio_mic_gain || 3) * 100) + '%' })),
        h('input', { type: 'range', min: '50', max: '800', step: '25', 'data-f': 'audio_mic_gain', 'data-k': 'audio_mic_gain', value: String(Math.round((ui.set.audio_mic_gain || 3) * 100)), 'aria-label': 'Volume del microfono', title: 'Se nel replay non si sente, alzalo. Vale dalla prossima registrazione. Il microfono registra tutto ciò che dici mentre la registrazione è attiva.' })))];
  });
}

function tickElapsed() {
  const el = $('elapsed');
  if (!el || !snap || !snap.match) return;
  const m = snap.match;
  el.textContent = m.phase === 'recording' && m.started_at_ms != null
    ? fmtTime(Date.now() + (m.clock_offset_ms ?? 0) - m.started_at_ms) : '';
}

const setRev = () => { region('v-settings', String(++ui.rev), buildSettings); updateSaveMsg(); };
function updateSaveMsg() { const e = $('savemsg'); if (e) e.textContent = ui.setDirty ? 'Salvataggio…' : ui.saveMsg; }

async function openSettings() {
  ui.view = 'settings';
  if (!ui.set) { try { ui.set = await invoke('get_settings'); } catch (e) { ui.notice = errMsg(e); } }
  render(snap);
  setRev();
}
async function closeSettings() {
  await flushSave();
  ui.view = 'main';
  render(snap);
}
async function refreshWindows() {
  ui.winBusy = true; ui.winErr = null;
  if (snap && snap.match) renderMatch();
  try { ui.windows = await invoke('list_windows'); } catch (e) { ui.winErr = errMsg(e); ui.windows = ui.windows || []; }
  ui.winBusy = false;
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
          field('Encoder', h('select', { 'data-f': 'encoder', 'data-k': 'encoder' }, ENCODERS.map(([v, t]) => h('option', { value: v, text: t, selected: s.encoder === v }))))))),
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
  back: () => closeSettings(),
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
  start: () => run('start', () => invoke('host_start', { force: false })),
  force: () => {
    if (!ui.forceArmed) {
      ui.forceArmed = true; renderMatch();
      clearTimeout(ACTIONS.force.t);
      ACTIONS.force.t = setTimeout(() => { ui.forceArmed = false; if (snap && snap.match) renderMatch(); }, 4000);
      const nb = document.querySelector('[data-act=force]'); if (nb) nb.focus();
      return;
    }
    ui.forceArmed = false;
    return run('start', () => invoke('host_start', { force: true }));
  },
  stop: () => run('stop', () => invoke('host_stop')),
  leave: () => run('leave', () => invoke('leave_match')),
  newmatch: () => run('leave', () => invoke('leave_match')),
  replay: () => invoke('open_url', { url: snap.match.replay_url }).catch((e) => { ui.notice = errMsg(e); renderBanner(); }),
  'refresh-windows': () => refreshWindows(),
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
