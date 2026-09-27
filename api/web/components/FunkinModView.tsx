"use client";

import Link from "next/link";
import { useMemo, useRef, useState } from "react";
import { ClipPlayer, GameBananaBox } from "@/components/FnfModView";
import {
  BASE_GAME_COVER,
  ENGINE_LABEL,
  BASE_GAME_KEY,
  FNF_LOGO,
  assetUrl,
  formatAccuracy,
  formatDay,
  formatScore,
  modTitle,
  plainVariation,
  type Engine,
} from "@/lib/codename";
import type { CodenameModDetail, CodenameSong, FunkinTrack } from "@/lib/types";

const trackKey = (id: string, variation?: string | null) => `${id.toLowerCase()}|${plainVariation(variation)}`;

function Stars({ n }: { n?: number }) {
  if (n == null) return null;
  return (
    <span className="fk-stars" title={`Difficoltà ${n}`}>
      <svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
        <path d="m12 2.5 2.9 6.1 6.6.8-4.9 4.6 1.3 6.6L12 17.3l-5.9 3.3 1.3-6.6-4.9-4.6 6.6-.8z" />
      </svg>
      {n}
    </span>
  );
}

function TrackCard({
  track,
  entries,
  art,
  engine,
  modKey,
}: {
  track: FunkinTrack;
  entries: CodenameSong[];
  art?: string;
  engine: Engine;
  modKey: string;
}) {
  const played = useMemo(() => new Map(entries.map((e) => [e.difficulty.toLowerCase(), e])), [entries]);
  const difficulties = useMemo(() => {
    const list = [...track.difficulties];
    for (const e of entries) if (!list.some((d) => d.toLowerCase() === e.difficulty.toLowerCase())) list.push(e.difficulty);
    return list;
  }, [track.difficulties, entries]);
  const latest = entries.reduce<CodenameSong | null>((a, e) => (!a || e.best.recorded_at > a.best.recorded_at ? e : a), null);
  const [diff, setDiff] = useState(latest?.difficulty.toLowerCase() ?? null);
  const [playing, setPlaying] = useState<string | null>(null);
  const [showArchive, setShowArchive] = useState(false);
  const entry = diff ? played.get(diff) : undefined;
  const current = entry ? ([entry.best, ...entry.archive].find((c) => c.id === playing) ?? entry.best) : null;

  const subtitle = [track.artist, track.bpm ? `${Math.round(track.bpm)} BPM` : null].filter(Boolean).join(" · ");

  return (
    <li className={"fnf-song" + (entries.length ? "" : " fk-todo")}>
      {current ? (
        <ClipPlayer engine={engine} clip={current} playing={playing === current.id} onPlay={() => setPlaying(current.id)} />
      ) : (
        <div
          className={"fnf-player thumb fk-empty" + (track.icon ? " fk-icon-bg" : "")}
          style={track.color ? { background: `linear-gradient(135deg, ${track.color}, color-mix(in srgb, ${track.color} 45%, #000))` } : undefined}
        >
          {track.icon ? (
            <span className="fk-healthicon">
              {/* eslint-disable-next-line @next/next/no-img-element */}
              <img src={assetUrl(engine, modKey, track.icon)} alt="" loading="lazy" />
            </span>
          ) : (
            // eslint-disable-next-line @next/next/no-img-element
            <img src={art ?? FNF_LOGO} alt="" loading="lazy" className={art ? "fk-empty-art" : "thumb-logo"} />
          )}
          <span className="fk-todo-label">{entries.length ? "Nessun record a questa difficoltà" : "Da giocare"}</span>
        </div>
      )}
      <div className="fnf-song-body">
        <div className="fnf-song-head">
          <div className="fnf-song-name">
            <div className="mcard-title" title={track.name}>{track.name}</div>
            {subtitle && <div className="fk-sub" title={subtitle}>{subtitle}</div>}
          </div>
          {entry && (
            <div className="fnf-score">
              <span>Record</span>
              <strong>{formatScore(entry.best.score)}</strong>
            </div>
          )}
        </div>
        {difficulties.length > 0 && (
          <div className="fk-diffs" role="tablist" aria-label="Difficoltà">
            {difficulties.map((d) => {
              const key = d.toLowerCase();
              const has = played.has(key);
              return (
                <button
                  key={key}
                  type="button"
                  role="tab"
                  aria-selected={diff === key}
                  className={"fk-diff" + (diff === key ? " on" : "") + (has ? " played" : "")}
                  onClick={() => {
                    setDiff(key);
                    setPlaying(null);
                    setShowArchive(false);
                  }}
                >
                  {d}
                  <Stars n={track.ratings[d] ?? track.ratings[key]} />
                </button>
              );
            })}
          </div>
        )}
        {entry && (
          <div className="fnf-stats">
            <span>
              Accuracy <b>{formatAccuracy(entry.best.accuracy)}</b>
            </span>
            <span>
              Note mancate <b>{entry.best.misses}</b>
            </span>
            <span>{formatDay(entry.best.recorded_at)}</span>
          </div>
        )}
        {entry && current && playing && playing !== entry.best.id && (
          <p className="fnf-note">
            Stai guardando un tentativo in archivio ({formatScore(current.score)}).{" "}
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

type Person = { name: string; role?: string; url?: string };

function Credits({ people }: { people: Person[] }) {
  const ref = useRef<HTMLDialogElement>(null);
  return (
    <>
      <button type="button" className="ghost small fk-credits-btn" onClick={() => ref.current?.showModal()}>
        Crediti · {people.length}
      </button>
      <dialog
        ref={ref}
        className="fk-credits"
        aria-labelledby="fk-credits-title"
        onClick={(e) => {
          if (e.target === ref.current) ref.current?.close();
        }}
      >
        <div className="fk-credits-inner">
          <div className="fk-credits-head">
            <h2 id="fk-credits-title">Crediti</h2>
            <button type="button" className="ghost small" onClick={() => ref.current?.close()} aria-label="Chiudi">
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true">
                <path d="M18 6 6 18M6 6l12 12" />
              </svg>
            </button>
          </div>
          <ul className="fk-people">
            {people.map((c, i) => {
              const inner = (
                <>
                  <strong>{c.name}</strong>
                  {c.role && <span>{c.role}</span>}
                </>
              );
              return (
                <li key={`${c.name}-${i}`}>
                  {c.url ? (
                    <a href={c.url} target="_blank" rel="noreferrer">
                      {inner}
                    </a>
                  ) : (
                    <div>{inner}</div>
                  )}
                </li>
              );
            })}
          </ul>
        </div>
      </dialog>
    </>
  );
}

type Group = { id: string; name: string; artists: string[]; art?: string; tracks: FunkinTrack[] };

export default function FunkinModView({ initial, engine = "funkin" }: { initial: CodenameModDetail; engine?: Engine }) {
  const [mod, setMod] = useState(initial.mod);
  const catalog = mod.catalog;
  const isBase = engine === "funkin" && mod.key === BASE_GAME_KEY;

  const { groups, byTrack, played, total } = useMemo(() => {
    const byTrack = new Map<string, CodenameSong[]>();
    for (const s of initial.songs) {
      const k = trackKey(s.song_id ?? s.song, s.variation);
      byTrack.set(k, [...(byTrack.get(k) ?? []), s]);
    }
    const tracks = [...(catalog?.tracks ?? [])];
    const known = new Set(tracks.map((t) => trackKey(t.id, t.variation)));
    for (const [k, songs] of byTrack) {
      if (known.has(k)) continue;
      known.add(k);
      tracks.push({
        id: songs[0].song_id ?? songs[0].song,
        variation: songs[0].variation,
        name: songs[0].song,
        difficulties: [],
        ratings: {},
      });
    }
    const albums = catalog?.albums ?? [];
    const groups: Group[] = albums
      .map((a) => ({ ...a, tracks: tracks.filter((t) => t.album === a.id) }))
      .filter((g) => g.tracks.length > 0);
    const albumIds = new Set(albums.map((a) => a.id));
    const rest = tracks.filter((t) => !t.album || !albumIds.has(t.album));
    if (rest.length) groups.push({ id: "", name: groups.length ? "Altre canzoni" : "Canzoni", artists: [], tracks: rest });
    const played = tracks.filter((t) => byTrack.has(trackKey(t.id, t.variation))).length;
    return { groups, byTrack, played, total: tracks.length };
  }, [initial.songs, catalog]);

  const icon = catalog?.has_icon ? assetUrl(engine, mod.key, "icon.png") : undefined;
  const photo = isBase ? BASE_GAME_COVER : mod.gb_cover_url;
  const cover = photo ?? icon;
  const contributors = catalog?.contributors ?? [];

  return (
    <div className="fnfmod">
      <Link href="/giochi/fnf" className="back">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
          <path d="m15 18-6-6 6-6" />
        </svg>
        Friday Night Funkin&apos;
      </Link>

      <header className="modhero">
        <div className={"modhero-cover" + (!photo && icon ? " fk-icon-cover" : "")}>
          {cover ? (
            <>
              {!photo && (
                // eslint-disable-next-line @next/next/no-img-element
                <img src={cover} alt="" aria-hidden="true" className="fk-cover-blur" />
              )}
              {/* eslint-disable-next-line @next/next/no-img-element */}
              <img src={cover} alt="" className={photo ? "" : "fk-cover-icon"} />
            </>
          ) : (
            // eslint-disable-next-line @next/next/no-img-element
            <img src={FNF_LOGO} alt="" className="logo-fallback" />
          )}
        </div>
        <div className="modhero-body">
          <span className="gametile-kicker">{isBase ? "Gioco base" : engine === "funkin" ? "Mod" : `Mod · ${ENGINE_LABEL[engine]}`}</span>
          <h1>{modTitle(mod)}</h1>
          <p className="muted">
            {mod.gb_author ? `di ${mod.gb_author} · ` : ""}
            {total ? `${played} di ${total} canzoni giocate` : `${initial.songs.length} record`}
            {catalog?.version ? ` · v${catalog.version.replace(/^v/i, "")}` : ""}
          </p>
          {catalog?.description && <p className="fk-desc">{catalog.description}</p>}
          <div className="fk-actions">
            {contributors.length > 0 && <Credits people={contributors} />}
            {!isBase && <GameBananaBox engine={engine} mod={mod} onChange={(m) => setMod({ ...m, catalog: m.catalog ?? catalog })} />}
          </div>
        </div>
      </header>

      {groups.map((g) => {
        const art = g.art ? assetUrl(engine, mod.key, g.art) : undefined;
        const done = g.tracks.filter((t) => byTrack.has(trackKey(t.id, t.variation))).length;
        return (
          <section key={g.id || "rest"} className="fk-album">
            <div className="fk-album-head">
              {art && (
                // eslint-disable-next-line @next/next/no-img-element
                <img src={art} alt="" loading="lazy" className={engine === "psych" || engine === "nmv" ? "fk-week-art" : "fk-album-art"} />
              )}
              <div>
                <h2>{g.name}</h2>
                <p className="muted fnf-hint">
                  {g.artists.length ? `${g.artists.join(", ")} · ` : ""}
                  {done} di {g.tracks.length} giocate
                </p>
              </div>
            </div>
            <ul className="fnf-songs">
              {g.tracks.map((t) => (
                <TrackCard
                  key={trackKey(t.id, t.variation)}
                  track={t}
                  entries={byTrack.get(trackKey(t.id, t.variation)) ?? []}
                  art={engine === "psych" || engine === "nmv" ? undefined : art}
                  engine={engine}
                  modKey={mod.key}
                />
              ))}
            </ul>
          </section>
        );
      })}
      {groups.length === 0 && <p className="muted">Nessuna canzone, per ora.</p>}
    </div>
  );
}
