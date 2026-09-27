"use client";

import Link from "next/link";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { clipThumb, clipVideo, formatAccuracy, formatDay, formatScore, type Engine } from "@/lib/codename";
import { customAsset } from "@/lib/custom-assets";
import type { CodenameClip, CodenameModDetail, CodenameSong } from "@/lib/types";

type Song = { name: string; id: string; char: number };
type World = { id: string; title: string; x: number; y: number; songs: Song[] };

const s = (name: string, id: string, char: number): Song => ({ name, id, char });

const WORLDS: World[] = [
  { id: "mainweek", title: "Mushroom Kingdom", x: 56.55, y: 130.5, songs: [s("It's a me", "its-a-me", 26), s("Starman Slaughter", "starman-slaughter", 34), s("All-Stars", "all-stars", 9)] },
  { id: "island", title: "Irregularity Island", x: 257.35, y: 130.5, songs: [s("So Cool", "so-cool", 8), s("Nourishing Blood", "nourishing-blood", 16), s("MARIO SING AND GAME RYTHM 9", "mario-sing-and-game-rythm-9", 13)] },
  {
    id: "woodland",
    title: "Woodland of Lies",
    x: 442.65,
    y: 130.5,
    songs: [s("Alone", "alone", 22), s("Oh God No", "oh-god-no", 21), s("I Hate You", "i-hate-you", 25), s("Thalassophobia", "thalassophobia", 31), s("Apparition", "apparition", 24), s("Last Course", "last-course", 18), s("Dark Forest", "dark-forest", 17)],
  },
  { id: "cosmos", title: "Content Cosmos", x: 638.55, y: 130.5, songs: [s("Bad Day", "bad-day", 12), s("Day Out", "day-out", 10), s("Dictator", "dictator", 11), s("Race-traitors", "racetraitors", 20), s("No Hope", "no-hope", 19)] },
  { id: "heights", title: "Hellish Heights", x: 833.8, y: 130.5, songs: [s("Golden Land", "golden-land", 28), s("No Party", "no-party", 30), s("Paranoia", "paranoia", 41), s("Overdue", "overdue", 35), s("Powerdown", "powerdown", 27), s("Demise", "demise", 23)] },
  { id: "classified", title: "Classified Castle", x: 1028, y: 130.5, songs: [s("Promotion", "promotion", 15), s("Abandoned", "abandoned", 32), s("The End", "the-end", 33)] },
  {
    id: "legacy",
    title: "Legacy Mode",
    x: 268.7,
    y: 472.35,
    songs: [
      s("It's a me (Original)", "its-a-me-old", 1),
      s("Golden Land (Original)", "golden-land-old", 2),
      s("I Hate You (Original)", "i-hate-you-old", 3),
      s("Powerdown (Original)", "powerdown-old", 4),
      s("Apparition (Original)", "apparition-old", 5),
      s("Forbidden Star", "forbidden-star", 39),
      s("Alone (Original)", "alone-old", 6),
      s("Race-traitors (Original)", "racetraitors-old", 7),
    ],
  },
  {
    id: "extra",
    title: "Extra Songs",
    x: 736.35,
    y: 472.35,
    songs: [
      s("Unbeatable", "unbeatable", 14),
      s("Dictator (Original)", "dictator-old", 42),
      s("No Party (Original)", "no-party-old", 36),
      s("Overdue (Original)", "overdue-old", 37),
      s("Time Out (Demise Original)", "demise-old", 40),
      s("All Stars Act 1 (Original)", "all-stars-old", 38),
    ],
  },
];

function songPath(name: string): string {
  return name
    .trim()
    .replace(/ /g, "-")
    .replace(/[~&\\;:<>#]/g, "-")
    .replace(/[.,'"%?!]/g, "")
    .toLowerCase();
}

export default function MariosMadnessView({
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
  const extra = (catalog?.extra ?? {}) as {
    logo?: string;
    ui?: { backgrounds?: string[]; static?: string[]; fog?: string };
  };
  const backgrounds = extra.ui?.backgrounds ?? [];
  const statics = extra.ui?.static ?? [];
  const asset = useCallback((name?: string | null) => (name ? customAsset("marios-madness", name) : undefined), []);

  const recordsFor = useCallback(
    (song: Song): CodenameSong[] =>
      initial.songs.filter((e) => {
        const id = (e.song_id ?? "").toLowerCase();
        return id === song.id || id === songPath(song.name) || songPath(e.song) === song.id || songPath(e.song) === songPath(song.name);
      }),
    [initial.songs],
  );

  const stageRef = useRef<HTMLDivElement>(null);
  const [scale, setScale] = useState(1);
  useEffect(() => {
    const el = stageRef.current;
    if (!el) return;
    const fit = () => setScale(Math.min(el.clientWidth / 1280, el.clientHeight / 720));
    fit();
    const ro = new ResizeObserver(fit);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const [bgIdx, setBgIdx] = useState(0);
  useEffect(() => {
    if (backgrounds.length < 2) return;
    const t = window.setTimeout(() => setBgIdx(Math.floor(Math.random() * backgrounds.length)), 0);
    return () => window.clearTimeout(t);
  }, [backgrounds.length]);
  const [staticFrame, setStaticFrame] = useState(0);
  useEffect(() => {
    if (statics.length < 2 || window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    const t = window.setInterval(() => setStaticFrame((f) => (f + 1) % statics.length), 1000 / 15);
    return () => window.clearInterval(t);
  }, [statics.length]);

  const [worldIdx, setWorldIdx] = useState<number | null>(null);
  const world = worldIdx == null ? null : WORLDS[worldIdx];
  const [sel, setSel] = useState(0);
  const [watching, setWatching] = useState<string | null>(null);
  const [hover, setHover] = useState<number | null>(null);

  const openWorld = (i: number) => {
    const w = WORLDS[i];
    setWorldIdx(i);
    setSel(Math.max(0, w.songs.findIndex((x) => recordsFor(x).length > 0)));
    setWatching(null);
  };
  const move = useCallback(
    (d: number) => {
      if (!world) return;
      setSel((v) => (v + d + world.songs.length) % world.songs.length);
      setWatching(null);
    },
    [world],
  );

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.target instanceof HTMLElement && ["INPUT", "TEXTAREA", "VIDEO"].includes(e.target.tagName)) return;
      if (world) {
        if (e.key === "ArrowLeft") move(-1);
        else if (e.key === "ArrowRight") move(1);
        else if (e.key === "Escape" || e.key === "Backspace") setWorldIdx(null);
        else return;
        e.preventDefault();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [world, move]);

  const song = world?.songs[sel];
  const entries = useMemo(() => (song ? recordsFor(song) : []), [song, recordsFor]);
  const best = entries.reduce<CodenameSong | null>((a, e) => (!a || e.best.score > a.best.score ? e : a), null);
  const clip: CodenameClip | undefined = best ? ([best.best, ...best.archive].find((c) => c.id === watching) ?? best.best) : undefined;
  const totalPlayed = WORLDS.reduce((n, w) => n + w.songs.filter((x) => recordsFor(x).length > 0).length, 0);
  const totalSongs = WORLDS.reduce((n, w) => n + w.songs.length, 0);

  return (
    <div className={`impx mmx ${fontClass}`}>
      <div className="impx-navspace mmx-navspace" aria-hidden="true" />
      <div className="impx-corner">
        <Link href="/giochi/fnf" className="impx-close" aria-label="Chiudi e torna a Friday Night Funkin'">
          ✕
        </Link>
        {extra.logo && (
          // eslint-disable-next-line @next/next/no-img-element
          <img src={asset(extra.logo)} alt={catalog?.title ?? mod.name} className="impx-cornerlogo" />
        )}
      </div>

      <div className="mmx-stage" ref={stageRef} style={{ ["--k" as string]: scale }}>
        <div className="mmx-bg" aria-hidden="true">
          {backgrounds[bgIdx] && (
            <div className="mmx-red">
              <div
                className="mmx-level"
                style={{
                  backgroundImage: `url(${asset(backgrounds[bgIdx])})`,
                  animationDuration: `${Math.round(20000 / (40 * Math.max(scale, 0.1)))}s`,
                }}
              />
            </div>
          )}
          {statics.length > 0 && (
            <div className="mmx-red mmx-static">
              {statics.map((f, i) => (
                // eslint-disable-next-line @next/next/no-img-element
                <img key={f} src={asset(f)} alt="" style={{ opacity: i === staticFrame ? 1 : 0 }} />
              ))}
            </div>
          )}
          {extra.ui?.fog && (
            // eslint-disable-next-line @next/next/no-img-element
            <img src={asset(extra.ui.fog)} alt="" className="mmx-fog" />
          )}
        </div>
        <div className="mmx-tv" aria-hidden="true" />
        <div className="mmx-screen" style={{ transform: `translate(-50%, -50%) scale(${scale})` }}>
          <div className="mmx-cam">
            {!world ? (
              <>
                {/* eslint-disable-next-line @next/next/no-img-element */}
                <img src={asset("mm-world-title.png")} alt="Select your destiny" className="mmx-title" />
                {WORLDS.map((w, i) => {
                  const done = w.songs.filter((x) => recordsFor(x).length > 0).length;
                  return (
                    <button
                      key={w.id}
                      type="button"
                      className={"mmx-world" + (hover === i ? " on" : "")}
                      style={{ left: w.x - 25, top: w.y - 15 }}
                      onMouseEnter={() => setHover(i)}
                      onMouseLeave={() => setHover(null)}
                      onFocus={() => setHover(i)}
                      onBlur={() => setHover(null)}
                      onClick={() => openWorld(i)}
                      aria-label={`${w.title}: ${done} di ${w.songs.length} canzoni giocate`}
                    >
                      {/* eslint-disable-next-line @next/next/no-img-element */}
                      <img src={asset(`mm-world-${w.id}.png`)} alt="" />
                      <span className="mmx-world-count">
                        {done}/{w.songs.length}
                      </span>
                    </button>
                  );
                })}
                <p className="mmx-desc below">
                  {hover != null ? WORLDS[hover].title : `${totalPlayed} di ${totalSongs} canzoni giocate`}
                </p>
              </>
            ) : (
              <>
                {/* eslint-disable-next-line @next/next/no-img-element */}
                <img src={asset("mm-sign.png")} alt="Select song" className="mmx-sign" />
                <div className="mmx-carousel" style={{ ["--sel" as string]: sel }}>
                  {world.songs.map((x, i) => (
                    <button
                      key={x.id}
                      type="button"
                      className={"mmx-char" + (i === sel ? " on" : "")}
                      style={{ ["--i" as string]: i }}
                      onClick={() => (i === sel ? undefined : setSel(i))}
                      aria-label={x.name}
                    >
                      {/* eslint-disable-next-line @next/next/no-img-element */}
                      <img src={asset(`mm-char-${x.char}.png`)} alt="" />
                    </button>
                  ))}
                </div>
                <button type="button" className="mmx-arrow left" onClick={() => move(-1)} aria-label="Canzone precedente">
                  {/* eslint-disable-next-line @next/next/no-img-element */}
                  <img src={asset("mm-arrow.png")} alt="" />
                </button>
                <button type="button" className="mmx-arrow right" onClick={() => move(1)} aria-label="Canzone successiva">
                  {/* eslint-disable-next-line @next/next/no-img-element */}
                  <img src={asset("mm-arrow.png")} alt="" />
                </button>

                <div className="mmx-record">
                  {clip ? (
                    <video
                      key={clip.id}
                      src={clip.processing ? undefined : clipVideo(clip.id, engine)}
                      poster={clip.has_thumb ? clipThumb(clip.id, engine) : undefined}
                      autoPlay
                      muted
                      loop
                      playsInline
                      controls
                    />
                  ) : (
                    <div className="mmx-norecord">DA GIOCARE</div>
                  )}
                  {best && (
                    <div className="mmx-stats">
                      <span>SCORE {formatScore(best.best.score)}</span>
                      <span>ACCURACY {formatAccuracy(best.best.accuracy)}</span>
                      <span>MISSES {best.best.misses}</span>
                      <span>{best.difficulty.toUpperCase()}</span>
                      <span>{formatDay(best.best.recorded_at).toUpperCase()}</span>
                    </div>
                  )}
                  {best && best.archive.length > 0 && (
                    <div className="mmx-archive">
                      {[best.best, ...best.archive].map((c, i) => (
                        <button key={c.id} type="button" className={c.id === clip?.id ? "on" : ""} onClick={() => setWatching(c.id)}>
                          {i === 0 ? "RECORD" : `#${i}`} {formatScore(c.score)}
                        </button>
                      ))}
                    </div>
                  )}
                </div>

                <p className="mmx-desc">{song?.name}</p>
                <button type="button" className="mmx-back" onClick={() => setWorldIdx(null)}>
                  ‹ {world.title.toUpperCase()}
                </button>
              </>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
