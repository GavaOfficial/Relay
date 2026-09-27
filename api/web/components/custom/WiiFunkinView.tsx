"use client";

import Link from "next/link";
import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { clipThumb, clipVideo, formatDay, formatScore, type Engine } from "@/lib/codename";
import { customAsset } from "@/lib/custom-assets";
import type { CodenameClip, CodenameModDetail, CodenameSong, FunkinTrack } from "@/lib/types";

type Channel = "wiik1" | "wiik2" | "wiikz" | "fisticuffs" | "whitegloves" | "freeplay" | "credits";
type Pack = "StoryMode" | "Bonus";
type View =
  | { at: "menu" }
  | { at: "channel"; ch: Channel; tile: number; leaving?: boolean }
  | { at: "packs" }
  | { at: "list"; pack: Pack; weeks?: string[] };

const GRID: (Channel | "")[][] = [
  ["wiik1", "wiik2", "wiikz", "fisticuffs"],
  ["whitegloves", "freeplay", "credits", ""],
  ["", "", "", ""],
];
const TILE_TEXT: Record<Channel, string> = {
  wiik1: "Wiik 1",
  wiik2: "Wiik 2",
  wiikz: "Wiik Z",
  fisticuffs: "Fisticuffs",
  whitegloves: "White Gloves",
  freeplay: "Freeplay",
  credits: "Credits",
};
const XPOS = [99, 373, 646, 919];
const CAN_EXIT: Record<Channel, number> = { wiik1: 1.95, wiik2: 1.95, wiikz: 3, fisticuffs: 2.25, whitegloves: 2.25, freeplay: 2, credits: 2 };
const YPOS = [60, 212, 365];
const CHANNEL_WEEK: Partial<Record<Channel, string>> = {
  wiik1: "wiik1",
  wiik2: "wiik2",
  wiikz: "wiikz",
  fisticuffs: "fridaynightfisticuffs",
  whitegloves: "wiikwhitegloves",
};
const PACK_WEEKS: Record<Pack, string> = {
  StoryMode: "wiik1|wiik2|wiikz|wiikwhitegloves|fridaynightfisticuffs",
  Bonus: "wiikbonus|stupidalts|shaggystuff|shaggyremixes",
};
const HIDDEN_SONGS = new Set(["five-hot", "funkin-corner"]);
const WEEK_PORT: Record<string, string> = {
  wiikwhitegloves: "wg",
  fridaynightfisticuffs: "fisticuffs",
  wiik1: "wiik1",
  wiik2: "wiik2",
  wiikz: "wiikz",
};
const SONG_PORT: Record<string, string> = {
  "basket-match": "hex",
  "sussy-qussy": "impostor",
  battlefield: "battlefield",
  swap: "swap",
  "mii-funkin": "miifunkin",
  "fired-up": "fired-up",
  miisacre: "miisacre",
  "paper-cut": "sketchy",
  lazulii: "lazulii",
  illusion: "illusion",
  heavenfall: "heavenfall",
  "long-awaited": "long-awaited",
  revolution: "revol",
  "3hot": "3hot",
  "4hot": "4hot",
  snacks: "shaggy",
  "motion-control": "eteled",
  destiny: "correrioptahsjdfklasdf",
  funkadelic: "nikku",
  broadcasting: "nikku",
  foulplay: "foul",
  "god-mode": "godmode",
  "boxing-match-wg": "bmwg",
  "funkin-corner": "corner",
};
const LABELS = new Set(["dlc", "fisticuffs", "freeplay", "whitegloves", "wiik1", "wiik2", "wiikz"]);
function labelOf(week: string) {
  const l = week === "wiikwhitegloves" ? "whitegloves" : week === "fridaynightfisticuffs" ? "fisticuffs" : week || "freeplay";
  return LABELS.has(l) ? l : "freeplay";
}

const STAR_X = [2, 79, 168, 276, 364];
const STAR_Y = [39, 25, 2, 24, 40];
const STAR_INFO = [
  "Record in tutte le canzoni di Wiik 1 e 2",
  "Record in tutte le canzoni di Wiik Z",
  "Record in tutte le canzoni di White Gloves",
  "Record in tutte le canzoni Bonus",
  "Record in tutte le canzoni di Fisticuffs",
];

const FP_PHOTOS = ["wiik-1", "wiik-2", "3hot", "wiik-z", "fisticuffs", "revolution", "mii-funkin"];
const FP_PHOTO_POS = [
  { x: -480, r: 4 },
  { x: -240, r: -6 },
  { x: 0, r: 8 },
  { x: 240, r: 3 },
  { x: 480, r: -8 },
];

type Row = { track: FunkinTrack; week: string; label: string; port: string; entries: CodenameSong[] };

export default function WiiFunkinView({ initial, engine, fontClass }: { initial: CodenameModDetail; engine: Engine; fontClass: string }) {
  const mod = initial.mod;
  const catalog = mod.catalog;
  const extra = (catalog?.extra ?? {}) as { logo?: string; ui?: { wii_weeks?: Record<string, string> } };
  const weekOf = useMemo(() => extra.ui?.wii_weeks ?? {}, [extra.ui?.wii_weeks]);
  const a = useCallback((name: string) => customAsset("wii-funkin", name), []);

  const rows: Row[] = useMemo(
    () =>
      (catalog?.tracks ?? [])
        .filter((t) => !HIDDEN_SONGS.has(t.id))
        .map((t) => {
          const week = weekOf[t.album ?? ""] ?? "";
          return {
            track: t,
            week,
            label: labelOf(week),
            port: SONG_PORT[t.id] ?? WEEK_PORT[week] ?? "",
            entries: initial.songs.filter((e) => (e.song_id ?? e.song).toLowerCase() === t.id),
          };
        }),
    [catalog, weekOf, initial.songs],
  );
  const played = useCallback((weeks: string[]) => {
    const list = rows.filter((r) => weeks.includes(r.week));
    return list.length > 0 && list.every((r) => r.entries.length > 0);
  }, [rows]);
  const stars = useMemo(
    () => [
      played(["wiik1", "wiik2"]),
      played(["wiikz"]),
      played(["wiikwhitegloves"]),
      rows.filter((r) => PACK_WEEKS.Bonus.includes(r.week)).every((r) => r.entries.length > 0),
      played(["fridaynightfisticuffs"]),
    ],
    [played, rows],
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

  const [view, setView] = useState<View>({ at: "menu" });
  const [muted, setMuted] = useState(true);
  const timers = useRef<number[]>([]);
  useEffect(() => () => timers.current.forEach((t) => window.clearTimeout(t)), []);
  const later = (ms: number, f: () => void) => timers.current.push(window.setTimeout(f, ms));
  const busy = useRef(false);

  const [wipe, setWipe] = useState<{ mode: "out" | "in" | "black"; n: number } | null>(null);
  const wipes = useRef(0);
  const go = (next: View, black = false) => {
    if (busy.current) return;
    busy.current = true;
    let t = 0;
    if (black) {
      later(250, () => setWipe({ mode: "black", n: ++wipes.current }));
      t = 750;
    } else {
      setWipe({ mode: "out", n: ++wipes.current });
    }
    later(t + 600, () => {
      resetCam();
      setView(next);
      setWipe({ mode: "in", n: ++wipes.current });
      later(700, () => {
        setWipe(null);
        busy.current = false;
      });
    });
  };

  const menuRef = useRef<HTMLDivElement>(null);
  const cam = useRef({ zoom: 1, sx: 0, sy: 0 });
  const raf = useRef(0);
  const applyCam = () => {
    const { zoom, sx, sy } = cam.current;
    const el = menuRef.current;
    if (el) el.style.transform = zoom === 1 && sx === 0 && sy === 0 ? "" : `translate(${640 * (1 - zoom) - zoom * sx}px, ${360 * (1 - zoom) - zoom * sy}px) scale(${zoom})`;
  };
  const resetCam = () => {
    cancelAnimationFrame(raf.current);
    cam.current = { zoom: 1, sx: 0, sy: 0 };
    applyCam();
  };
  const moveCam = (to: { zoom: number; sx: number; sy: number }, scrollEase: (x: number) => number) => {
    cancelAnimationFrame(raf.current);
    const from = { ...cam.current };
    let start = -1;
    const step = (now: number) => {
      if (start < 0) start = now;
      const k = Math.min(1, (now - start) / 500);
      const e = scrollEase(k);
      cam.current = { zoom: from.zoom + (to.zoom - from.zoom) * k, sx: from.sx + (to.sx - from.sx) * e, sy: from.sy + (to.sy - from.sy) * e };
      applyCam();
      if (k < 1) raf.current = requestAnimationFrame(step);
    };
    raf.current = requestAnimationFrame(step);
  };
  useEffect(() => () => cancelAnimationFrame(raf.current), []);

  const canExit = useRef(false);
  const openChannel = (ch: Channel, tile: number) => {
    if (busy.current || view.at !== "menu") return;
    setView({ at: "channel", ch, tile });
    canExit.current = false;
    later(CAN_EXIT[ch] * 1000, () => (canExit.current = true));
    const cx = XPOS[tile % 4] + 129;
    const cy = YPOS[Math.floor(tile / 4)] + 70;
    later(250, () => moveCam({ zoom: 4, sx: cx - 640, sy: cy - 360 }, (x) => 1 - (1 - x) ** 4));
  };
  const leaveChannel = () => {
    if (view.at !== "channel" || view.leaving || busy.current) return;
    if (!canExit.current) return;
    setView({ ...view, leaving: true });
    moveCam({ zoom: 1, sx: 0, sy: 0 }, (x) => x ** 4);
    later(500, () => setView({ at: "menu" }));
  };
  const startChannel = () => {
    if (view.at !== "channel" || view.leaving) return;
    if (view.ch === "credits") return;
    else if (view.ch === "freeplay") go({ at: "packs" }, true);
    else go({ at: "list", pack: "StoryMode", weeks: [CHANNEL_WEEK[view.ch]!] }, true);
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" && e.key !== "Backspace") return;
      if (e.target instanceof HTMLElement && ["INPUT", "TEXTAREA"].includes(e.target.tagName)) return;
      if (view.at === "channel") leaveChannel();
      else if (view.at === "packs") go({ at: "menu" });
      else if (view.at === "list") go(view.weeks ? { at: "menu" } : { at: "packs" });
      else return;
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const chTile = view.at === "channel" ? view.tile : -1;
  const tileX = chTile >= 0 ? XPOS[chTile % 4] + 129 : 640;
  const tileY = chTile >= 0 ? YPOS[Math.floor(chTile / 4)] + 70 : 360;

  return (
    <div className={`impx wiix ${fontClass}`}>
      <div className="impx-navspace wiix-navspace" aria-hidden="true" />
      <div className="impx-corner">
        <Link href="/giochi/fnf" className="impx-close" aria-label="Chiudi e torna a Friday Night Funkin'">
          ✕
        </Link>
        {extra.logo && (
          // eslint-disable-next-line @next/next/no-img-element
          <img src={a(extra.logo)} alt={catalog?.title ?? mod.name} className="impx-cornerlogo" />
        )}
      </div>

      <div className="mmx-stage wiix-stage" ref={stageRef}>
        <div className="mmx-screen wiix-screen" style={{ transform: `translate(-50%, -50%) scale(${scale})` }}>
          {(view.at === "menu" || view.at === "channel") && (
            <>
              <div ref={menuRef} className={`wiix-menu${view.at === "channel" ? " in-channel" : ""}`}>
                {/* eslint-disable-next-line @next/next/no-img-element */}
                <img src={a("wii-bg.png")} alt="" className="wiix-full wiix-menubg" />
                {GRID.flatMap((row, j) =>
                  row.map((ch, i) => {
                    const pos: CSSProperties = { left: XPOS[i], top: YPOS[j] };
                    const n = j * 4 + i;
                    return (
                      <div key={n} className="wiix-slot" style={pos}>
                        {/* eslint-disable-next-line @next/next/no-img-element */}
                        <img src={a("wii-outline.png")} alt="" className="wiix-outline" />
                        {ch ? (
                          <button type="button" className="wiix-tile" onClick={() => openChannel(ch, n)} disabled={view.at !== "menu"}>
                            {/* eslint-disable-next-line @next/next/no-img-element */}
                            <img src={a(`wii-menu-${ch}.png`)} alt={TILE_TEXT[ch]} />
                            {/* eslint-disable-next-line @next/next/no-img-element */}
                            <img src={a("wii-outline-blue.png")} alt="" className="wiix-outline-blue" />
                            <span className="wiix-tip wiix-tip-tile">{TILE_TEXT[ch]}</span>
                          </button>
                        ) : (
                          <div className="wiix-tile wiix-null">
                            {/* eslint-disable-next-line @next/next/no-img-element */}
                            <img src={a("wii-menu-null.png")} alt="" />
                          </div>
                        )}
                      </div>
                    );
                  }),
                )}
                <button type="button" className="wiix-round" style={{ left: 40, top: 536 }} onClick={() => setMuted((m) => !m)}>
                  {/* eslint-disable-next-line @next/next/no-img-element */}
                  <img src={a("wii-options.png")} alt="" />
                  <span className="wiix-tip" style={{ left: 25, top: -60 }}>
                    {muted ? "Audio dei record: spento" : "Audio dei record: acceso"}
                  </span>
                </button>
                {/* La bacheca dei messaggi del gioco: qui solo come nel menu, senza link. */}
                <span className="wiix-round" style={{ left: 1088, top: 536 }}>
                  {/* eslint-disable-next-line @next/next/no-img-element */}
                  <img src={a("wii-messages.png")} alt="" />
                  <span className="wiix-tip" style={{ left: -80, top: -60 }}>
                    Messages
                  </span>
                </span>
                {stars.map((on, i) => (
                  <span key={i} className={`wiix-star${on ? " on" : ""}`} style={{ left: 415 + STAR_X[i], top: 544 + STAR_Y[i] }}>
                    {/* eslint-disable-next-line @next/next/no-img-element */}
                    <img src={a(`wii-star-${i}.png`)} alt="" />
                    <span className="wiix-tip" style={{ left: "50%", top: -60, translate: "-50% 0" }}>
                      {STAR_INFO[i]}
                    </span>
                  </span>
                ))}
                {view.at === "channel" && (
                  <div className="wiix-chworld" style={{ left: tileX - 640, top: tileY - 360 }}>
                    <ChannelScreen key={view.ch} ch={view.ch} a={a} leaving={!!view.leaving} onMenu={leaveChannel} onStart={startChannel} canStart={view.ch !== "credits"} />
                  </div>
                )}
              </div>
            </>
          )}

          {view.at === "packs" && <PackScreen a={a} onPick={(pack) => go({ at: "list", pack })} onBack={() => go({ at: "menu" })} />}

          {view.at === "list" && (
            <ListScreen
              key={`${view.pack}-${view.weeks?.join() ?? ""}`}
              a={a}
              engine={engine}
              muted={muted}
              pack={view.pack}
              rows={rows.filter((r) => (view.weeks ? view.weeks.includes(r.week) : r.week && PACK_WEEKS[view.pack].includes(r.week)))}
              onBack={() => go(view.weeks ? { at: "menu" } : { at: "packs" })}
            />
          )}
          {wipe && <div key={wipe.n} className={`wiix-wipe ${wipe.mode}`} aria-hidden="true" />}
        </div>
      </div>
    </div>
  );
}

function ChannelScreen({
  ch,
  a,
  leaving,
  onMenu,
  onStart,
  canStart,
}: {
  ch: Channel;
  a: (n: string) => string;
  leaving: boolean;
  onMenu: () => void;
  onStart: () => void;
  canStart: boolean;
}) {
  const [photos] = useState(() => {
    const pool = [...FP_PHOTOS];
    return FP_PHOTO_POS.map(() => pool.splice(Math.floor(Math.random() * pool.length), 1)[0]);
  });
  const bg = ch === "credits" ? null : `wii-ch-${ch}.png`;
  return (
    <div className={`wiix-channel ch-${ch}${leaving ? " leaving" : ""}`}>
      {/* Le quattro dissolvenze bianche intorno al canale (whiteFades). */}
      <div className="wiix-chwhitefade" aria-hidden="true" />
      {ch === "credits" ? <div className="wiix-full wiix-chwhite" /> : null}
      {bg && (
        <div className="wiix-chbg">
          {/* eslint-disable-next-line @next/next/no-img-element */}
          <img src={a(bg)} alt="" />
        </div>
      )}
      {ch === "wiikz" && (
        <>
          <div className="wiix-full wiix-chblack" />
          <Sprite a={a} n="wii-ch-z.png" w={913} h={443} dx={40} dy={-100} cls="z-num" />
          <div className="wiix-full wiix-chflash" />
          <Sprite a={a} n="wii-ch-wiik_z.png" w={913} h={443} dx={-12} dy={-84} cls="z-wiik" />
        </>
      )}
      {ch === "wiik1" && (
        <>
          <Sprite a={a} n="wii-ch-1.png" w={913} h={443} dx={60} dy={-100} cls="w1-num" />
          <Sprite a={a} n="wii-ch-wiik.png" w={913} h={443} dx={60} dy={-100} cls="w1-wiik" />
        </>
      )}
      {ch === "wiik2" && (
        <>
          <Sprite a={a} n="wii-ch-2.png" w={913} h={443} dx={40} dy={-80} cls="w2-num" />
          <Sprite a={a} n="wii-ch-wiik_2.png" w={913} h={443} dx={40} dy={-80} cls="w2-wiik" />
        </>
      )}
      {ch === "fisticuffs" && (
        <>
          <Sprite a={a} n="wii-ch-fisticuffs_logo.png" w={620} h={486} dx={-240} dy={-80} cls="fc-logo" />
          <Sprite a={a} n="wii-ch-fisticuffs_matt_render.png" w={428} h={581} dx={320} dy={-40} cls="fc-render" />
        </>
      )}
      {ch === "whitegloves" && <Sprite a={a} n="wii-ch-wg.png" w={800} h={604} dx={0} dy={-48} cls="wg-logo" />}
      {ch === "freeplay" && (
        <>
          {[0, 3, 4].map((i) => (
            <FpPhoto key={i} a={a} name={photos[i]} i={i} />
          ))}
        </>
      )}
      {ch === "credits" && (
        <>
          <Sprite a={a} n="wii-ch-creditsmiisback.png" w={1280} h={381} dx={0} dy={48} cls="cr-back" />
          <Sprite a={a} n="wii-ch-creditswhitefade.png" w={1280} h={381} dx={0} dy={48} cls="cr-fade" />
          <Sprite a={a} n="wii-ch-creditsmiisfront.png" w={1280} h={250} dx={0} dy={108} cls="cr-front" />
        </>
      )}
      {/* eslint-disable-next-line @next/next/no-img-element */}
      <img src={a("wii-ch-channelborder.png")} alt="" className="wiix-full wiix-chborder" />
      {ch === "freeplay" && (
        <>
          {[1, 2].map((i) => (
            <FpPhoto key={i} a={a} name={photos[i]} i={i} />
          ))}
          <Sprite a={a} n="wii-chfp-freeplay.png" w={576} h={91} dx={0} dy={-220} cls="fp-item" />
          <Sprite a={a} n="wii-chfp-wf.png" w={213} h={41} dx={440} dy={-288} cls="fp-item" />
          <Sprite a={a} n="wii-chfp-text.png" w={852} h={74} dx={0} dy={128} cls="fp-item" />
        </>
      )}
      {ch === "credits" && <Sprite a={a} n="wii-ch-creditslogo.png" w={536} h={130} dx={0} dy={-220} cls="cr-logo" />}
      <div className="wiix-chbuttons">
        <button type="button" className="wiix-chbtn" style={{ left: 400 - 184.5 }} onClick={onMenu}>
          <span>Menu</span>
        </button>
        <button type="button" className="wiix-chbtn" style={{ left: 880 - 184.5 }} onClick={onStart} disabled={!canStart}>
          <span>Start</span>
        </button>
      </div>
      <style>{`.wiix-chbtn{background-image:url("${a("wii-button.png")}")}.wiix-chbtn:hover:not(:disabled){background-image:url("${a("wii-button-on.png")}")}`}</style>
    </div>
  );
}

function Sprite({ a, n, w, h, dx, dy, cls }: { a: (n: string) => string; n: string; w: number; h: number; dx: number; dy: number; cls: string }) {
  return (
    // eslint-disable-next-line @next/next/no-img-element
    <img src={a(n)} alt="" className={`wiix-sprite ${cls}`} style={{ left: 640 - w / 2 + dx, top: 360 - h / 2 + dy, width: w, height: h }} />
  );
}

function FpPhoto({ a, name, i }: { a: (n: string) => string; name: string; i: number }) {
  const p = FP_PHOTO_POS[i];
  return (
    // eslint-disable-next-line @next/next/no-img-element
    <img
      src={a(`wii-chfp-${name}.png`)}
      alt=""
      className="wiix-sprite fp-item"
      style={{ left: 640 - 165.5 + p.x, top: 360 - 119.5 - 32, width: 331, height: 239, rotate: `${p.r}deg` }}
    />
  );
}

function PackScreen({ a, onPick, onBack }: { a: (n: string) => string; onPick: (p: Pack) => void; onBack: () => void }) {
  return (
    <div className="wiix-packs">
      {/* eslint-disable-next-line @next/next/no-img-element */}
      <img src={a("wii-fp-bg.png")} alt="" className="wiix-full" />
      <p className="wiix-question">Which songs would you like to play?</p>
      {(["StoryMode", "Bonus", "DLC"] as const).map((p, i) => (
        <button
          key={p}
          type="button"
          className="wiix-cover"
          style={{ left: 640 - 160 + 350 * (i - 1) }}
          onClick={() => p !== "DLC" && onPick(p)}
          disabled={p === "DLC"}
          aria-label={p}
        >
          {/* eslint-disable-next-line @next/next/no-img-element */}
          <img src={a(`wii-cover-${p.toLowerCase()}.png`)} alt="" />
        </button>
      ))}
      <p className="wiix-branding">2025 Matt Team</p>
      <BackButton a={a} onBack={onBack} />
    </div>
  );
}

function BackButton({ a, onBack }: { a: (n: string) => string; onBack: () => void }) {
  return (
    <button type="button" className="wiix-round wiix-back" onClick={onBack} aria-label="Indietro">
      {/* eslint-disable-next-line @next/next/no-img-element */}
      <img src={a("wii-back.png")} alt="" />
    </button>
  );
}

function ListScreen({
  a,
  engine,
  muted,
  pack,
  rows,
  onBack,
}: {
  a: (n: string) => string;
  engine: Engine;
  muted: boolean;
  pack: Pack;
  rows: Row[];
  onBack: () => void;
}) {
  const [sel, setSel] = useState(() => Math.max(0, rows.findIndex((r) => r.entries.length > 0)));
  const [scroll, setScroll] = useState(() => Math.max(0, Math.min(sel, rows.length - 4)));
  const [diff, setDiff] = useState(0);
  const [watching, setWatching] = useState<string | null>(null);

  const select = useCallback(
    (i: number, keepScroll = false) => {
      if (!rows.length) return;
      const n = (i + rows.length) % rows.length;
      setSel(n);
      setDiff(0);
      setWatching(null);
      if (!keepScroll) setScroll((s) => (n > s + 3 ? n - 3 : n < s ? n : s));
    },
    [rows.length],
  );
  const row = rows[sel];
  const entries = useMemo(() => [...(row?.entries ?? [])].sort((x, y) => y.best.score - x.best.score), [row]);
  const entry = entries[diff % Math.max(1, entries.length)];
  const clip: CodenameClip | undefined = entry ? ([entry.best, ...entry.archive].find((c) => c.id === watching) ?? entry.best) : undefined;

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.target instanceof HTMLElement && ["INPUT", "TEXTAREA", "VIDEO"].includes(e.target.tagName)) return;
      const shift = e.shiftKey ? 3 : 1;
      if (e.key === "ArrowUp") select(sel - shift);
      else if (e.key === "ArrowDown") select(sel + shift);
      else if (e.key === "Home") select(0);
      else if (e.key === "End") select(rows.length - 1);
      else if ((e.key === "ArrowLeft" || e.key === "ArrowRight") && entries.length > 1) {
        setDiff((d) => (d + (e.key === "ArrowLeft" ? -1 : 1) + entries.length) % entries.length);
        setWatching(null);
      } else return;
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [select, sel, rows.length, entries.length]);

  const onWheel = (e: React.WheelEvent) => {
    const d = Math.sign(e.deltaY) * (e.shiftKey ? 3 : 1);
    setScroll((s) => Math.max(0, Math.min(Math.max(0, rows.length - 4), s + d)));
  };

  const ports = useMemo(() => [...new Set(rows.map((r) => r.port).filter(Boolean))], [rows]);
  const fc = (r: Row) => r.entries.some((e) => e.best.misses === 0);

  return (
    <div className="wiix-list" onWheel={onWheel}>
      {/* eslint-disable-next-line @next/next/no-img-element */}
      <img src={a("wii-fp-bg.png")} alt="" className="wiix-full" />
      {ports.map((p) => (
        // eslint-disable-next-line @next/next/no-img-element
        <img
          key={p}
          src={a(`wii-port-${p}.png`)}
          alt=""
          className={`wiix-port${p === row?.port ? " on" : ""}${p === row?.port && !row.entries.length ? " hidden" : ""}`}
        />
      ))}

      <div className="wiix-rows" style={{ ["--scroll" as string]: scroll - 1 }}>
        {rows.map((r, i) => (
          <button
            key={r.track.id}
            type="button"
            className={`wiix-row${i === sel ? " on" : ""}`}
            style={{ ["--i" as string]: i, ["--mask" as string]: `url("${a(`wii-label-${r.label}.png`)}")` }}
            onMouseEnter={() => i !== sel && select(i, true)}
            onFocus={() => i !== sel && select(i, true)}
            onClick={() => select(i, true)}
          >
            {/* eslint-disable-next-line @next/next/no-img-element */}
            <img src={a(`wii-label-${r.label}.png`)} alt="" className="wiix-label" />
            <span className="wiix-tint" aria-hidden="true" />
            {fc(r) && (
              // eslint-disable-next-line @next/next/no-img-element
              <img src={a("wii-label-fcbadge.png")} alt="Full combo" className="wiix-label" />
            )}
            {r.entries.length > 0 && r.track.icon && (
              <span className="wiix-icon" style={{ backgroundImage: `url("${a(r.track.icon)}")` }} aria-hidden="true" />
            )}
            <span className="wiix-song">{r.track.name}</span>
          </button>
        ))}
      </div>

      {clip && (
        <div className="wiix-record">
          <video
            key={clip.id}
            src={clipVideo(clip.id, engine)}
            poster={clip.has_thumb ? clipThumb(clip.id, engine) : undefined}
            autoPlay
            muted={muted}
            loop
            playsInline
            controls
          />
          {entry && entry.archive.length > 0 && (
            <div className="wiix-archive">
              {[entry.best, ...entry.archive].map((c, i) => (
                <button key={c.id} type="button" className={c.id === clip.id ? "on" : ""} onClick={() => setWatching(c.id)}>
                  {i === 0 ? "Record" : `#${i}`} · {formatScore(c.score)}
                </button>
              ))}
            </div>
          )}
        </div>
      )}

      {/* eslint-disable-next-line @next/next/no-img-element */}
      <img src={a("wii-fp-bar.png")} alt="" className="wiix-modebar" />
      {/* eslint-disable-next-line @next/next/no-img-element */}
      <img src={a(pack === "StoryMode" ? "wii-fp-story.png" : "wii-fp-bonus.png")} alt={pack === "StoryMode" ? "Story Mode Songs" : "Bonus Songs"} className="wiix-modetitle" />
      <BackButton a={a} onBack={onBack} />

      <div className="wiix-score">
        <span>
          Personal Best: {entry ? entry.best.score : 0} ({entry?.best.accuracy != null ? (entry.best.accuracy * 100).toFixed(2) : "0.00"}%)
        </span>
        <span>
          {entry
            ? entries.length > 1
              ? `< ${entry.difficulty} >`
              : `${entry.difficulty} · ${entry.best.misses} miss · ${formatDay(entry.best.recorded_at)}`
            : "Nessun record"}
        </span>
      </div>
      <p className="wiix-bottom">
        Frecce SU/GIU&apos; o il mouse per scegliere la canzone / SINISTRA e DESTRA per la difficolta&apos; / ESC per tornare indietro
      </p>
    </div>
  );
}
