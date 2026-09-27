"use client";

import Link from "next/link";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { clipThumb, clipVideo, formatAccuracy, formatDay, formatScore, plainVariation, type Engine } from "@/lib/codename";
import { customAsset } from "@/lib/custom-assets";
import type { CodenameClip, CodenameModDetail, CodenameSong } from "@/lib/types";

type Section = { id: string; title: string; index?: number | null; icon?: string | null };
type TrackExtra = { portrait?: string; composers?: string; section?: string };
type Ui = {
  star_bg?: string;
  star_fg?: string;
  top_bar?: string;
  card?: string;
  glow?: string;
  note?: string;
  back?: string;
  key_left?: string;
  key_right?: string;
};

const keyOf = (id: string, variation?: string | null) => `${id.toLowerCase()}|${plainVariation(variation)}`;

function rankOf(accuracy?: number | null, misses = 0): { text: string; color: string } {
  if (accuracy == null) return { text: "", color: "#fff" };
  const acc = accuracy * 100;
  if (acc >= 100 && misses === 0) return { text: "P", color: "#eda3f7" };
  if (acc > 99 && misses === 0) return { text: "P", color: "#00ff00" };
  if (acc > 95 && misses === 0) return { text: "S", color: "#ffff00" };
  if (acc > 90 && misses <= 20) return { text: "A", color: "#fff" };
  if (acc > 85 && misses <= 30) return { text: "B", color: "#fff" };
  if (acc > 70 && misses <= 50) return { text: "C", color: "#fff" };
  if (acc > 50 && misses <= 70) return { text: "D", color: "#fff" };
  return { text: "F", color: "#ff0000" };
}

export default function ImpostorLegacyView({
  initial,
  engine,
  fontClass,
}: {
  initial: CodenameModDetail;
  engine: Engine;
  fontClass: string;
}) {
  const mod = initial.mod;
  const catalog = mod.catalog;
  const extra = (catalog?.extra ?? {}) as { logo?: string; sections?: Section[]; ui?: Ui };
  const ui = extra.ui ?? {};
  const asset = useCallback((name?: string | null) => (name ? customAsset("impostor-legacy", name) : undefined), []);
  const tracks = useMemo(() => catalog?.tracks ?? [], [catalog]);

  const byTrack = useMemo(() => {
    const m = new Map<string, CodenameSong[]>();
    for (const s of initial.songs) {
      const k = keyOf(s.song_id ?? s.song, s.variation);
      m.set(k, [...(m.get(k) ?? []), s]);
    }
    return m;
  }, [initial.songs]);

  const sections = useMemo(() => {
    const list = [...(extra.sections ?? [])].sort((a, b) => (a.index ?? 99) - (b.index ?? 99));
    const inSection = (id: string) => tracks.filter((t) => (t.extra as TrackExtra | undefined)?.section === id);
    const out = list.map((s) => ({ ...s, tracks: inSection(s.id) })).filter((s) => s.tracks.length > 0);
    const known = new Set(list.map((s) => s.id));
    const rest = tracks.filter((t) => !known.has((t.extra as TrackExtra | undefined)?.section ?? ""));
    if (rest.length) out.push({ id: "altro", title: "Altre canzoni", index: 99, icon: null, tracks: rest });
    return out;
  }, [extra.sections, tracks]);

  const [sectionIdx, setSectionIdx] = useState(() =>
    Math.max(0, sections.findIndex((s) => s.tracks.some((t) => byTrack.has(keyOf(t.id, t.variation))))),
  );
  const section = sections[sectionIdx] ?? sections[0];
  const songs = useMemo(() => section?.tracks ?? [], [section]);
  const [sel, setSel] = useState(() => Math.max(0, songs.findIndex((t) => byTrack.has(keyOf(t.id, t.variation)))));
  const track = songs[Math.min(sel, songs.length - 1)];
  const entries = useMemo(() => (track ? (byTrack.get(keyOf(track.id, track.variation)) ?? []) : []), [track, byTrack]);
  const [diff, setDiff] = useState<string | null>(null);
  const [watching, setWatching] = useState<string | null>(null);
  const [archiveOpen, setArchiveOpen] = useState(false);

  const byDiff = useMemo(() => new Map(entries.map((e) => [e.difficulty.toLowerCase(), e])), [entries]);
  const difficulties = useMemo(() => {
    const list = track ? [...track.difficulties] : [];
    for (const e of entries) if (!list.some((d) => d.toLowerCase() === e.difficulty.toLowerCase())) list.push(e.difficulty);
    return list;
  }, [track, entries]);
  const latest = entries.reduce<CodenameSong | null>((a, e) => (!a || e.best.recorded_at > a.best.recorded_at ? e : a), null);
  const currentDiff = diff ?? latest?.difficulty.toLowerCase() ?? difficulties[0]?.toLowerCase() ?? "";
  const entry = byDiff.get(currentDiff);
  const clip: CodenameClip | undefined = entry ? ([entry.best, ...entry.archive].find((c) => c.id === watching) ?? entry.best) : undefined;
  const trackExtra = (track?.extra ?? {}) as TrackExtra;

  const select = useCallback(
    (i: number) => {
      if (!songs.length) return;
      setSel(((i % songs.length) + songs.length) % songs.length);
      setDiff(null);
      setWatching(null);
      setArchiveOpen(false);
    },
    [songs.length],
  );
  const changeSection = useCallback(
    (to: number) => {
      const n = ((to % sections.length) + sections.length) % sections.length;
      setSectionIdx(n);
      setSel(0);
      setDiff(null);
      setWatching(null);
      setArchiveOpen(false);
    },
    [sections.length],
  );

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.target instanceof HTMLElement && ["INPUT", "TEXTAREA", "VIDEO"].includes(e.target.tagName)) return;
      if (e.key === "ArrowUp") select(sel - 1);
      else if (e.key === "ArrowDown") select(sel + 1);
      else if (e.key === "ArrowLeft") changeSection(sectionIdx - 1);
      else if (e.key === "ArrowRight") changeSection(sectionIdx + 1);
      else return;
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [sel, sectionIdx, select, changeSection]);

  const wheelAcc = useRef(0);
  const onWheel = (e: React.WheelEvent) => {
    wheelAcc.current += e.deltaY;
    if (Math.abs(wheelAcc.current) >= 60) {
      select(sel + Math.sign(wheelAcc.current));
      wheelAcc.current = 0;
    }
  };

  return (
    <div className={`impx ${fontClass}`} style={{ ["--song" as string]: track?.color ?? "#ff0000" }}>
      <div className="impx-space" aria-hidden="true">
        {ui.star_bg && <div className="impx-stars impx-stars-bg" style={{ backgroundImage: `url(${asset(ui.star_bg)})` }} />}
        {ui.star_fg && <div className="impx-stars impx-stars-fg" style={{ backgroundImage: `url(${asset(ui.star_fg)})` }} />}
      </div>

      {/* In alto la barra classica di Relay (quella del sito) e il logo della mod nell'angolo. */}
      <div
        className="impx-navspace"
        aria-hidden="true"
        style={ui.top_bar ? { backgroundImage: `url(${asset(ui.top_bar)})` } : undefined}
      />
      <div className="impx-corner">
        <Link href="/giochi/fnf" className="impx-close" aria-label="Chiudi e torna a Friday Night Funkin'">
          {ui.back ? (
            // eslint-disable-next-line @next/next/no-img-element
            <img src={asset(ui.back)} alt="" />
          ) : (
            "✕"
          )}
        </Link>
        {extra.logo && (
          // eslint-disable-next-line @next/next/no-img-element
          <img src={asset(extra.logo)} alt={catalog?.title ?? mod.name} className="impx-cornerlogo" />
        )}
      </div>

      <div className="impx-stage">
        <div className="impx-glow" aria-hidden="true" />
        {trackExtra.portrait && (
          // eslint-disable-next-line @next/next/no-img-element
          <img
            key={trackExtra.portrait}
            src={asset(trackExtra.portrait)}
            alt=""
            className={"impx-portrait" + (clip ? " behind" : "") + (entries.length ? "" : " todo")}
          />
        )}

        <ul className="impx-wheel" onWheel={onWheel} aria-label="Canzoni">
          {songs.map((t, i) => {
            const dist = i - sel;
            if (Math.abs(dist) > 4) return null;
            const es = byTrack.get(keyOf(t.id, t.variation)) ?? [];
            const best = es.reduce<CodenameSong | null>((a, e) => (!a || e.best.score > a.best.score ? e : a), null);
            const r = best ? rankOf(best.best.accuracy, best.best.misses) : null;
            const composers = (t.extra as TrackExtra | undefined)?.composers;
            return (
              <li
                key={t.id}
                className="impx-card"
                style={{
                  ["--dist" as string]: dist,
                  ["--adist" as string]: Math.abs(dist),
                  opacity: Math.max(0, 1 - Math.abs(dist) * 0.25),
                  zIndex: 10 - Math.abs(dist),
                }}
              >
                <button
                  type="button"
                  onClick={() => select(i)}
                  aria-current={dist === 0}
                  style={ui.card ? { backgroundImage: `url(${asset(ui.card)})` } : undefined}
                >
                  <span className="impx-card-name">{t.name}</span>
                  {composers && (
                    <span className="impx-card-credit">
                      {ui.note && (
                        // eslint-disable-next-line @next/next/no-img-element
                        <img src={asset(ui.note)} alt="" />
                      )}
                      {composers}
                    </span>
                  )}
                  {r && r.text && (
                    <span className="impx-card-rank" style={{ color: r.color }}>
                      {r.text}
                    </span>
                  )}
                </button>
                {t.icon && (
                  <span className="impx-card-icon" aria-hidden="true">
                    {/* eslint-disable-next-line @next/next/no-img-element */}
                    <img src={asset(t.icon)} alt="" />
                  </span>
                )}
              </li>
            );
          })}
        </ul>

        {track && (
          <div className="impx-info">
            <p className="impx-line">
              {entry
                ? `SCORE: ${formatScore(entry.best.score)} | ACCURACY: ${formatAccuracy(entry.best.accuracy)}`
                : entries.length
                  ? "NESSUN RECORD A QUESTA DIFFICOLTA'"
                  : "DA GIOCARE"}
            </p>
            {entry && (
              <p className="impx-line small">
                MISSES: {entry.best.misses} | {formatDay(entry.best.recorded_at).toUpperCase()}
              </p>
            )}
            <p className="impx-line small dim">
              {[trackExtra.composers && `DI ${trackExtra.composers.toUpperCase()}`, track.bpm ? `${Math.round(track.bpm)} BPM` : null]
                .filter(Boolean)
                .join(" | ")}
            </p>
            {difficulties.length > 1 && (
              <div className="impx-diffs">
                {difficulties.map((d) => (
                  <button
                    key={d}
                    type="button"
                    className={(d.toLowerCase() === currentDiff ? "on" : "") + (byDiff.has(d.toLowerCase()) ? " played" : "")}
                    onClick={() => {
                      setDiff(d.toLowerCase());
                      setWatching(null);
                    }}
                  >
                    {d}
                  </button>
                ))}
              </div>
            )}
          </div>
        )}

        {clip && (
          <div className="impx-clip">
            <video
              key={clip.id}
              src={clipVideo(clip.id, engine)}
              poster={clip.has_thumb ? clipThumb(clip.id, engine) : undefined}
              autoPlay
              muted
              loop
              playsInline
              controls
            />
            {entry && entry.archive.length > 0 && (
              <div className="impx-archive">
                <button type="button" className="impx-archive-toggle" onClick={() => setArchiveOpen((v) => !v)} aria-expanded={archiveOpen}>
                  {watching && watching !== entry.best.id ? "TENTATIVO IN ARCHIVIO" : "RECORD"} · ARCHIVIO ({entry.archive.length})
                </button>
                {archiveOpen && (
                  <ul>
                    {[entry.best, ...entry.archive].map((c, i) => (
                      <li key={c.id}>
                        <button type="button" className={c.id === clip.id ? "on" : ""} onClick={() => setWatching(c.id)}>
                          {i === 0 ? "RECORD " : ""}
                          {formatScore(c.score)} | {formatAccuracy(c.accuracy)} | {formatDay(c.recorded_at)}
                        </button>
                      </li>
                    ))}
                  </ul>
                )}
              </div>
            )}
          </div>
        )}

        <nav className="impx-tabs" aria-label="Sezioni">
          {sections.map((s, i) => {
            const done = s.tracks.filter((t) => byTrack.has(keyOf(t.id, t.variation))).length;
            return (
              <button
                key={s.id}
                type="button"
                className={"impx-tab" + (i === sectionIdx ? " on" : "")}
                aria-pressed={i === sectionIdx}
                title={s.title}
                onClick={() => changeSection(i)}
              >
                {s.icon ? (
                  // eslint-disable-next-line @next/next/no-img-element
                  <img src={asset(s.icon)} alt={s.title} />
                ) : (
                  <span>{s.title}</span>
                )}
                <small>
                  {done}/{s.tracks.length}
                </small>
              </button>
            );
          })}
        </nav>

      </div>
    </div>
  );
}
