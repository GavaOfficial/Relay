"use client";

import Link from "next/link";
import { useState } from "react";
import {
  FNF_LOGO,
  clipThumb,
  clipVideo,
  formatAccuracy,
  formatClipLength,
  formatDay,
  formatScore,
  modTitle,
  type Engine,
} from "@/lib/codename";
import type { CodenameClip, CodenameMod, CodenameModDetail, CodenameSong } from "@/lib/types";

export function ClipPlayer({
  clip,
  playing,
  onPlay,
  engine = "codename",
}: {
  clip: CodenameClip;
  playing: boolean;
  onPlay: () => void;
  engine?: Engine;
}) {
  if (playing && clip.processing) {
    return (
      <div className="fnf-player fnf-processing">
        <p>La clip è in elaborazione: sarà pronta tra poco.</p>
      </div>
    );
  }
  if (playing) {
    return (
      <div className="fnf-player">
        <video src={clipVideo(clip.id, engine)} controls autoPlay playsInline preload="metadata" />
      </div>
    );
  }
  return (
    <button type="button" className="fnf-player thumb" onClick={onPlay} aria-label={`Guarda ${clip.song}`}>
      {clip.has_thumb ? (
        // eslint-disable-next-line @next/next/no-img-element
        <img src={clipThumb(clip.id, engine)} alt="" loading="lazy" />
      ) : (
        // eslint-disable-next-line @next/next/no-img-element
        <img src={FNF_LOGO} alt="" className="thumb-logo" />
      )}
      <span className="fnf-play" aria-hidden="true">
        <svg width="22" height="22" viewBox="0 0 24 24" fill="currentColor">
          <path d="M8 5.5v13a1 1 0 0 0 1.5.86l10.5-6.5a1 1 0 0 0 0-1.72L9.5 4.64A1 1 0 0 0 8 5.5z" />
        </svg>
      </span>
      <span className="badge">{clip.processing ? "In elaborazione" : formatClipLength(clip.duration_ms)}</span>
    </button>
  );
}

function SongCard({ entry }: { entry: CodenameSong }) {
  const [playing, setPlaying] = useState<string | null>(null);
  const [showArchive, setShowArchive] = useState(false);
  const current = [entry.best, ...entry.archive].find((c) => c.id === playing) ?? entry.best;
  const best = entry.best;

  return (
    <li className="fnf-song">
      <ClipPlayer clip={current} playing={playing === current.id} onPlay={() => setPlaying(current.id)} />
      <div className="fnf-song-body">
        <div className="fnf-song-head">
          <div className="fnf-song-name">
            <div className="mcard-title" title={entry.song}>{entry.song}</div>
            <span className="pill">{entry.difficulty}</span>
          </div>
          <div className="fnf-score">
            <span>Record</span>
            <strong>{formatScore(best.score)}</strong>
          </div>
        </div>
        <div className="fnf-stats">
          <span>
            Accuracy <b>{formatAccuracy(best.accuracy)}</b>
          </span>
          <span>
            Note mancate <b>{best.misses}</b>
          </span>
          <span>{formatDay(best.recorded_at)}</span>
        </div>
        {playing && playing !== best.id && (
          <p className="fnf-note">
            Stai guardando un tentativo in archivio ({formatScore(current.score)}).{" "}
            <button type="button" className="linkish" onClick={() => setPlaying(best.id)}>
              Torna al record
            </button>
          </p>
        )}
        {entry.archive.length > 0 && (
          <>
            <button type="button" className="linkish fnf-archive-toggle" onClick={() => setShowArchive((v) => !v)} aria-expanded={showArchive}>
              {showArchive ? "Nascondi archivio" : `Archivio (${entry.archive.length})`}
            </button>
            {showArchive && (
              <ul className="fnf-archive">
                {entry.archive.map((c) => (
                  <li key={c.id}>
                    <button type="button" className={playing === c.id ? "on" : ""} onClick={() => setPlaying(c.id)}>
                      <span className="fnf-archive-score">{formatScore(c.score)}</span>
                      <span className="muted">{formatAccuracy(c.accuracy)}</span>
                      <span className="muted">{formatDay(c.recorded_at)}</span>
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </>
        )}
      </div>
    </li>
  );
}

export const linkSite = (url?: string) => (url && /(^|\/\/|\.)gamejolt\.com\//.test(url) ? "Game Jolt" : "GameBanana");

export function GameBananaBox({
  mod,
  onChange,
  engine = "codename",
}: {
  mod: CodenameMod;
  onChange: (m: CodenameMod) => void;
  engine?: Engine;
}) {
  const [editing, setEditing] = useState(false);
  const [url, setUrl] = useState(mod.gamebanana_url ?? "");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  const save = async (value: string) => {
    setBusy(true);
    setErr(null);
    try {
      const r = await fetch(`/api/${engine}/mods/${encodeURIComponent(mod.key)}/gamebanana`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ url: value }),
      });
      if (!r.ok) {
        setErr(r.status === 400 ? await r.text() : "Non sono riuscito a salvare il link.");
        return;
      }
      onChange((await r.json()) as CodenameMod);
      setEditing(false);
    } catch {
      setErr("Il server non risponde.");
    } finally {
      setBusy(false);
    }
  };

  if (!editing) {
    return (
      <div className="gb-actions">
        {mod.gamebanana_url && (
          <a href={mod.gamebanana_url} target="_blank" rel="noreferrer" className="gb-link">
            Apri su {linkSite(mod.gamebanana_url)}
          </a>
        )}
        <button type="button" className="ghost small" onClick={() => { setUrl(mod.gamebanana_url ?? ""); setEditing(true); }}>
          {mod.gamebanana_url ? "Cambia link" : "Collega GameBanana o Game Jolt"}
        </button>
      </div>
    );
  }
  return (
    <form
      className="gb-form"
      onSubmit={(e) => {
        e.preventDefault();
        void save(url.trim());
      }}
    >
      <input
        autoFocus
        type="url"
        value={url}
        onChange={(e) => setUrl(e.target.value)}
        placeholder="https://gamebanana.com/mods/…"
        aria-label="Link della mod su GameBanana o Game Jolt"
      />
      <button type="submit" disabled={busy || !url.trim()}>{busy ? "Carico…" : "Salva"}</button>
      {mod.gamebanana_url && (
        <button type="button" className="ghost" disabled={busy} onClick={() => void save("")}>Rimuovi</button>
      )}
      <button type="button" className="ghost" disabled={busy} onClick={() => setEditing(false)}>Annulla</button>
      {err && <p className="err">{err}</p>}
      <p className="muted fnf-hint">Prendo in automatico nome, autore e copertina della mod.</p>
    </form>
  );
}

export default function FnfModView({ initial }: { initial: CodenameModDetail }) {
  const [mod, setMod] = useState(initial.mod);
  const records = initial.songs.length;

  return (
    <div className="fnfmod">
      <Link href="/giochi/fnf" className="back">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
          <path d="m15 18-6-6 6-6" />
        </svg>
        Friday Night Funkin&apos;
      </Link>

      <header className="modhero">
        <div className="modhero-cover">
          {/* eslint-disable-next-line @next/next/no-img-element */}
          <img src={mod.gb_cover_url ?? FNF_LOGO} alt="" className={mod.gb_cover_url ? "" : "logo-fallback"} />
        </div>
        <div className="modhero-body">
          <span className="gametile-kicker">Mod · Codename Engine</span>
          <h1>{modTitle(mod)}</h1>
          <p className="muted">
            {mod.gb_author ? `di ${mod.gb_author} · ` : ""}
            {records === 1 ? "1 record" : `${records} record`}
          </p>
          <GameBananaBox mod={mod} onChange={setMod} />
        </div>
      </header>

      <div className="section-title">
        <h2>Canzoni</h2>
        <span className="muted fnf-hint">il tuo miglior punteggio per ogni difficoltà</span>
      </div>
      <ul className="fnf-songs">
        {initial.songs.map((s) => (
          <SongCard key={`${s.song}-${s.difficulty}`} entry={s} />
        ))}
      </ul>
    </div>
  );
}
