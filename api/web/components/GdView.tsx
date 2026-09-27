"use client";

import Link from "next/link";
import { useMemo, useState } from "react";
import { clipThumb, clipVideo, formatClipLength, formatDay } from "@/lib/codename";
import {
  GD_LOGO,
  difficultyFace,
  formatNumber,
  gdIcon,
  levelThumb,
  playerIcon,
  resultText,
  type GdAttempt,
  type GdLevel,
  type GdProfile,
} from "@/lib/gd";
import type { CodenameClip, CodenameModDetail, CodenameSong, FunkinTrack } from "@/lib/types";

const attemptOf = (c: CodenameClip) => (c.extra ?? {}) as GdAttempt;

function LevelImage({ id, clip }: { id: string; clip?: CodenameClip }) {
  const sources = [levelThumb(id), clip?.has_thumb ? clipThumb(clip.id, "gd") : null, GD_LOGO].filter(Boolean) as string[];
  const [i, setI] = useState(0);
  const src = sources[Math.min(i, sources.length - 1)];
  return (
    // eslint-disable-next-line @next/next/no-img-element
    <img
      src={src}
      alt=""
      loading="lazy"
      className={src === GD_LOGO ? "thumb-logo" : ""}
      onError={() => setI((n) => n + 1)}
    />
  );
}

function LevelCard({ track, entry }: { track: FunkinTrack; entry?: CodenameSong }) {
  const level = (track.extra ?? {}) as GdLevel;
  const platformer = !!level.platformer || track.difficulties.includes("platformer");
  const face = difficultyFace(level);
  const [playing, setPlaying] = useState<string | null>(null);
  const [showArchive, setShowArchive] = useState(false);
  const best = entry?.best;
  const current = entry ? ([entry.best, ...entry.archive].find((c) => c.id === playing) ?? entry.best) : undefined;
  const a = best ? attemptOf(best) : undefined;
  const coinsTotal = level.coins ?? 0;
  const coinIcon = level.type === 1 ? "coin" : "silvercoin";

  return (
    <li className="fnf-song gd-level">
      {current && playing === current.id ? (
        <div className="fnf-player">
          {current.processing ? (
            <p className="fnf-processing-text">La clip è in elaborazione: sarà pronta tra poco.</p>
          ) : (
            <video src={clipVideo(current.id, "gd")} controls autoPlay playsInline preload="metadata" />
          )}
        </div>
      ) : (
        <button
          type="button"
          className="fnf-player thumb gd-thumb"
          onClick={() => current && setPlaying(current.id)}
          disabled={!current}
          aria-label={current ? `Guarda ${track.name}` : track.name}
        >
          <LevelImage id={track.id} clip={best} />
          {current && (
            <span className="fnf-play" aria-hidden="true">
              <svg width="22" height="22" viewBox="0 0 24 24" fill="currentColor">
                <path d="M8 5.5v13a1 1 0 0 0 1.5.86l10.5-6.5a1 1 0 0 0 0-1.72L9.5 4.64A1 1 0 0 0 8 5.5z" />
              </svg>
            </span>
          )}
          {current && <span className="badge">{formatClipLength(current.duration_ms)}</span>}
        </button>
      )}
      <div className="fnf-song-body">
        <div className="fnf-song-head">
          {/* eslint-disable-next-line @next/next/no-img-element */}
          <img src={face.src} alt={face.label} title={face.label} className="gd-face" />
          <div className="fnf-song-name">
            <div className="mcard-title" title={track.name}>{track.name}</div>
            <div className="fk-sub">
              {track.artist ? `di ${track.artist}` : level.type === 1 ? "RobTop" : "Livello"}
              {(level.stars ?? 0) > 0 && (
                <span className="gd-stars">
                  {" · "}
                  {level.stars}
                  {/* eslint-disable-next-line @next/next/no-img-element */}
                  <img src={gdIcon(platformer ? "moon" : "star")} alt={platformer ? "lune" : "stelle"} />
                </span>
              )}
            </div>
          </div>
          <div className="fnf-score">
            <span>{platformer ? "Miglior tempo" : "Record"}</span>
            <strong>{resultText(a, platformer)}</strong>
          </div>
        </div>
        {coinsTotal > 0 && (
          <div className="gd-coins" aria-label={`Monete: ${a?.coins ?? 0} di ${coinsTotal}`}>
            {Array.from({ length: coinsTotal }, (_, i) => (
              // eslint-disable-next-line @next/next/no-img-element
              <img key={i} src={gdIcon(coinIcon)} alt="" className={i < (a?.coins ?? 0) ? "" : "off"} />
            ))}
          </div>
        )}
        <div className="fnf-stats">
          {level.attempts != null && (
            <span>
              Tentativi <b>{formatNumber(level.attempts)}</b>
            </span>
          )}
          {level.jumps != null && (
            <span>
              Salti <b>{formatNumber(level.jumps)}</b>
            </span>
          )}
          {best && <span>{formatDay(best.recorded_at)}</span>}
          {!best && <span>Nessun record ancora</span>}
        </div>
        {entry && current && playing && playing !== entry.best.id && (
          <p className="fnf-note">
            Stai guardando un tentativo in archivio ({resultText(attemptOf(current), platformer)}).{" "}
            <button type="button" className="linkish" onClick={() => setPlaying(entry.best.id)}>
              Torna al record
            </button>
          </p>
        )}
        {entry && entry.archive.length > 0 && (
          <>
            <button type="button" className="linkish fnf-archive-toggle" onClick={() => setShowArchive((v) => !v)} aria-expanded={showArchive}>
              {showArchive ? "Nascondi archivio" : `Archivio (${entry.archive.length})`}
            </button>
            {showArchive && (
              <ul className="fnf-archive">
                {entry.archive.map((c) => (
                  <li key={c.id}>
                    <button type="button" className={playing === c.id ? "on" : ""} onClick={() => setPlaying(c.id)}>
                      <span className="fnf-archive-score">{resultText(attemptOf(c), platformer)}</span>
                      <span className="muted">tentativo {attemptOf(c).attempt ?? "—"}</span>
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

function Section({ detail }: { detail: CodenameModDetail }) {
  const { tracks, byId } = useMemo(() => {
    const byId = new Map(detail.songs.map((s) => [s.song_id ?? s.song, s]));
    const known = detail.mod.catalog?.tracks ?? [];
    const ids = new Set(known.map((t) => t.id));
    const orphans: FunkinTrack[] = detail.songs
      .filter((s) => !ids.has(s.song_id ?? s.song))
      .map((s) => ({ id: s.song_id ?? s.song, name: s.song, difficulties: [s.difficulty], ratings: {} }));
    const last = (t: FunkinTrack) => byId.get(t.id)?.best.recorded_at ?? 0;
    const tracks = [...known, ...orphans].sort((a, b) => last(b) - last(a));
    return { tracks, byId };
  }, [detail]);
  if (tracks.length === 0) return null;
  return (
    <section className="fk-album">
      <div className="section-title">
        <h2>{detail.mod.name}</h2>
        <span className="muted fnf-hint">
          {tracks.length === 1 ? "1 livello" : `${tracks.length} livelli`}
        </span>
      </div>
      <ul className="fnf-songs">
        {tracks.map((t) => (
          <LevelCard key={t.id} track={t} entry={byId.get(t.id)} />
        ))}
      </ul>
    </section>
  );
}

function Stat({ icon, label, value }: { icon: string; label: string; value?: number }) {
  return (
    <div className="gd-stat" title={label}>
      {/* eslint-disable-next-line @next/next/no-img-element */}
      <img src={gdIcon(icon)} alt="" />
      <strong>{formatNumber(value)}</strong>
      <span>{label}</span>
    </div>
  );
}

export default function GdView({ profile, sections }: { profile: GdProfile | null; sections: CodenameModDetail[] }) {
  const empty = sections.every((s) => s.songs.length === 0 && !(s.mod.catalog?.tracks.length));
  return (
    <div className="fnfmod">
      <Link href="/giochi" className="back">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
          <path d="m15 18-6-6 6-6" />
        </svg>
        Giochi
      </Link>

      <header className="modhero gd-hero">
        <div className="gd-player">
          {profile?.username ? (
            // eslint-disable-next-line @next/next/no-img-element
            <img src={playerIcon(profile.username)} alt="" />
          ) : (
            // eslint-disable-next-line @next/next/no-img-element
            <img src={GD_LOGO} alt="" className="logo-fallback" />
          )}
        </div>
        <div className="modhero-body">
          <span className="gametile-kicker">Geometry Dash</span>
          <h1>{profile?.username ?? "Geometry Dash"}</h1>
          {profile ? (
            <div className="gd-stats">
              <Stat icon="star" label="Stelle" value={profile.stars} />
              <Stat icon="moon" label="Lune" value={profile.moons} />
              <Stat icon="demon" label="Demon" value={profile.demons} />
              <Stat icon="coin" label="Monete segrete" value={profile.secret_coins} />
              <Stat icon="silvercoin" label="Monete utente" value={profile.user_coins} />
              <Stat icon="diamond" label="Diamanti" value={profile.diamonds} />
            </div>
          ) : (
            <p className="muted">Il tuo profilo compare qui dopo la prima partita con Relay collegato.</p>
          )}
        </div>
      </header>

      {empty ? (
        <div className="emptystate">
          {/* eslint-disable-next-line @next/next/no-img-element */}
          <img src={GD_LOGO} alt="" className="empty-logo" />
          <p>Nessun livello, per ora.</p>
          <p className="muted">
            Nell&apos;app Relay vai in Impostazioni → Connettori → Geometry Dash e collega il gioco (serve Geode).
            Ogni nuovo record diventa una clip qui.
          </p>
        </div>
      ) : (
        sections.map((d) => <Section key={d.mod.key} detail={d} />)
      )}
    </div>
  );
}
