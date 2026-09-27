"use client";

import Link from "next/link";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { clipThumb, clipVideo, formatDay, formatScore, type Engine } from "@/lib/codename";
import { customAsset } from "@/lib/custom-assets";
import type { CodenameClip, CodenameModDetail, CodenameSong } from "@/lib/types";

type Strip = { file: string; frames: number; w: number; h: number };
type Ui = { letters?: Record<string, Strip>; icons?: Record<string, Strip> };

const TYPES = ["story", "bonus", "nightmare"] as const;
type FType = 0 | 1 | 2;
const SONGS: [string, string][][] = [
  [
    ["snake-eyes", "cuphead"],
    ["technicolor-tussle", "cuphead"],
    ["knockout", "angrycuphead"],
    ["whoopee", "sans"],
    ["sansational", "sans"],
    ["burning-in-hell", "sansscared"],
    ["final-stretch", "sans"],
    ["imminent-demise", "bendyda"],
    ["terrible-sin", "bendy"],
    ["last-reel", "bendy"],
    ["nightmare-run", "bendy"],
  ],
  [
    ["satanic-funkin", "devilfull"],
    ["bad-to-the-bone", "papyrus"],
    ["bonedoggle", "papyrusandsans"],
    ["ritual", "sammy"],
    ["freaky-machine", "bendyda"],
  ],
  [
    ["devils-gambit", "cupheadnightmare"],
    ["bad-time", "sansnightmare"],
    ["despair", "bendynightmare"],
  ],
];
const LOCKED_DIFF: Record<string, string> = {
  "nightmare-run": "HARD",
  "final-stretch": "HARD",
  "burning-in-hell": "HARD",
  "bad-time": "GENOCIDAL",
  "devils-gambit": "DEVILISH",
  despair: "DEMONIC",
};
const ICON_OFFSET: Record<string, [number, number]> = { angrycuphead: [0, 15], sansnightmare: [20, 30] };

const songTitle = (id: string) => id.replace(/-/g, " ");

export default function IndieCrossView({ initial, engine, fontClass }: { initial: CodenameModDetail; engine: Engine; fontClass: string }) {
  const mod = initial.mod;
  const catalog = mod.catalog;
  const extra = (catalog?.extra ?? {}) as { logo?: string; ui?: Ui };
  const ui = extra.ui ?? {};
  const a = useCallback((name: string) => customAsset("indie-cross", name), []);
  const bpms = useMemo(() => Object.fromEntries((catalog?.tracks ?? []).map((t) => [t.id, t.bpm ?? 120])), [catalog]);

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

  const [screen, setScreen] = useState<"select" | FType>("select");
  const [trans, setTrans] = useState<{ dir: "out" | "in"; n: number } | null>(null);
  const transN = useRef(0);
  const timers = useRef<number[]>([]);
  useEffect(() => () => timers.current.forEach((t) => window.clearTimeout(t)), []);
  const later = (ms: number, f: () => void) => timers.current.push(window.setTimeout(f, ms));
  const busy = useRef(false);
  const switchTo = (next: "select" | FType, wait = 0) => {
    if (busy.current) return;
    busy.current = true;
    later(wait, () => {
      setTrans({ dir: "out", n: ++transN.current });
      later(500, () => {
        setScreen(next);
        setTrans({ dir: "in", n: ++transN.current });
        later(500, () => {
          setTrans(null);
          busy.current = false;
        });
      });
    });
  };

  return (
    <div className={`impx icx ${fontClass}`}>
      <div className="impx-navspace icx-navspace" aria-hidden="true" />
      <div className="impx-corner">
        <Link href="/giochi/fnf" className="impx-close" aria-label="Chiudi e torna a Friday Night Funkin'">
          ✕
        </Link>
        {extra.logo && (
          // eslint-disable-next-line @next/next/no-img-element
          <img src={a(extra.logo)} alt={catalog?.title ?? mod.name} className="impx-cornerlogo" />
        )}
      </div>
      <div className="mmx-stage icx-stage" ref={stageRef}>
        <div className="mmx-screen icx-screen" style={{ transform: `translate(-50%, -50%) scale(${scale})` }}>
          {screen === "select" ? (
            <SelectScreen a={a} onPick={(t) => switchTo(t, 1000)} />
          ) : (
            <ListScreen key={`list-${screen}`} a={a} ui={ui} engine={engine} type={screen} songs={initial.songs} bpms={bpms} onBack={() => switchTo("select", 500)} />
          )}
          {trans && <Diamonds key={`trans-${trans.n}`} dir={trans.dir} />}
        </div>
      </div>
    </div>
  );
}

function SelectScreen({ a, onPick }: { a: (n: string) => string; onPick: (t: FType) => void }) {
  const [sel, setSel] = useState<FType>(0);
  const [picked, setPicked] = useState<FType | null>(null);
  const pick = (t: FType) => {
    if (picked !== null) return;
    setSel(t);
    setPicked(t);
    onPick(t);
  };
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (picked !== null) return;
      if (e.key === "ArrowLeft" || e.key === "a") setSel((s) => ((s + 2) % 3) as FType);
      else if (e.key === "ArrowRight" || e.key === "d") setSel((s) => ((s + 1) % 3) as FType);
      else if (e.key === "Enter") pick(sel);
      else return;
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });
  return (
    <div className="icx-select">
      {/* eslint-disable-next-line @next/next/no-img-element */}
      <img src={a("ic-bg.png")} alt="" className="icx-full" />
      {TYPES.map((t, i) => (
        <button
          key={t}
          type="button"
          className={`icx-selbtn${i === sel ? " on" : ""}${picked === i ? " picked" : ""}${picked !== null && picked !== i ? " gone" : ""}`}
          style={{ left: 120 + i * 332, ["--mask" as string]: `url("${a(`ic-select-${t}.png`)}")` }}
          onMouseEnter={() => picked === null && setSel(i as FType)}
          onClick={() => pick(i as FType)}
          aria-label={`${t} songs`}
        >
          {/* eslint-disable-next-line @next/next/no-img-element */}
          <img src={a(`ic-select-${t}.png`)} alt="" />
          <span className="icx-white" aria-hidden="true" />
        </button>
      ))}
    </div>
  );
}

function useWord(text: string, letters: Record<string, Strip>) {
  return useMemo(() => {
    let x = 0;
    let spaces = 0;
    const out: { ch: string; x: number; s: Strip }[] = [];
    for (const c of text.toUpperCase()) {
      if (c === " ") {
        spaces++;
        continue;
      }
      const s = letters[c];
      if (!s) continue;
      x += 40 * spaces;
      spaces = 0;
      out.push({ ch: c, x, s });
      x += s.w;
    }
    return { glyphs: out, width: x };
  }, [text, letters]);
}

function Word({ text, letters, a }: { text: string; letters: Record<string, Strip>; a: (n: string) => string }) {
  const { glyphs, width } = useWord(text, letters);
  return (
    <span className="icx-word" style={{ width }}>
      {glyphs.map((g, i) => (
        <Anim key={i} s={g.s} a={a} style={{ left: g.x }} />
      ))}
    </span>
  );
}

function Anim({ s, a, style }: { s: Strip; a: (n: string) => string; style?: React.CSSProperties }) {
  return (
    <span
      className={`icx-anim${s.frames > 1 ? " play" : ""}`}
      style={{
        ...style,
        width: s.w,
        height: s.h,
        backgroundImage: `url("${a(s.file)}")`,
        ["--n" as string]: s.frames,
        ["--w" as string]: `${s.w}px`,
        ["--dur" as string]: `${s.frames / 24}s`,
      }}
      aria-hidden="true"
    />
  );
}

type Row = { id: string; icon: string; entries: CodenameSong[] };

function ListScreen({
  a,
  ui,
  engine,
  type,
  songs,
  bpms,
  onBack,
}: {
  a: (n: string) => string;
  ui: Ui;
  engine: Engine;
  type: FType;
  songs: CodenameSong[];
  bpms: Record<string, number>;
  onBack: () => void;
}) {
  const letters = ui.letters ?? {};
  const icons = ui.icons ?? {};
  const rows: Row[] = useMemo(
    () =>
      SONGS[type].map(([id, icon]) => ({
        id,
        icon,
        entries: songs.filter((e) => (e.song_id ?? e.song).toLowerCase().replace(/ /g, "-") === id),
      })),
    [type, songs],
  );
  const [sel, setSel] = useState(() => Math.max(0, rows.findIndex((r) => r.entries.length > 0)));
  const [diff, setDiff] = useState(0);
  const [watching, setWatching] = useState<string | null>(null);
  const [leaving, setLeaving] = useState(false);
  const change = useCallback(
    (d: number) => {
      setSel((s) => (s + d + rows.length) % rows.length);
      setDiff(0);
      setWatching(null);
    },
    [rows.length],
  );
  const row = rows[sel];
  const entries = useMemo(() => [...(row?.entries ?? [])].sort((x, y) => y.best.score - x.best.score), [row]);
  const entry = entries.length ? entries[diff % entries.length] : undefined;
  const clip: CodenameClip | undefined = entry ? ([entry.best, ...entry.archive].find((c) => c.id === watching) ?? entry.best) : undefined;
  const locked = row ? LOCKED_DIFF[row.id] : undefined;
  const diffName = locked ?? (entry?.difficulty ?? "Normal").toUpperCase();

  const back = () => {
    if (leaving) return;
    setLeaving(true);
    onBack();
  };
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.target instanceof HTMLElement && ["INPUT", "TEXTAREA", "VIDEO"].includes(e.target.tagName)) return;
      if (e.key === "ArrowUp") change(-1);
      else if (e.key === "ArrowDown") change(1);
      else if ((e.key === "ArrowLeft" || e.key === "ArrowRight") && entries.length > 1 && !locked) {
        setDiff((d) => (d + (e.key === "ArrowLeft" ? -1 : 1) + entries.length) % entries.length);
        setWatching(null);
      } else if (e.key === "Escape" || e.key === "Backspace") back();
      else return;
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const rowRefs = useRef<(HTMLDivElement | null)[]>([]);
  const pos = useRef<{ x: number; y: number }[]>([]);
  const selRef = useRef(sel);
  useEffect(() => {
    selRef.current = sel;
  }, [sel]);
  useEffect(() => {
    let raf = 0;
    let last = performance.now();
    pos.current = rows.map((_, i) => ({ x: 90 + (i - selRef.current) * 20, y: (i - selRef.current) * 156 + 345.6 }));
    const step = (now: number) => {
      const k = Math.min(1, ((now - last) / 1000) * 9.6);
      last = now;
      rows.forEach((_, i) => {
        const t = i - selRef.current;
        const p = pos.current[i];
        p.x += (t * 20 + 90 - p.x) * k;
        p.y += (t * 156 + 345.6 - p.y) * k;
        const el = rowRefs.current[i];
        if (el) el.style.transform = `translate(${p.x}px, ${p.y}px)`;
      });
      raf = requestAnimationFrame(step);
    };
    raf = requestAnimationFrame(step);
    return () => cancelAnimationFrame(raf);
  }, [rows]);

  const camRef = useRef<HTMLDivElement>(null);
  const bpm = bpms[row?.id ?? ""] ?? 120;
  useEffect(() => {
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    const cam = camRef.current;
    if (!cam) return;
    let beat = 0;
    const t = window.setInterval(() => {
      beat++;
      const shake = row?.id === "bad-time" && beat % 2 === 0;
      cam.animate(
        [
          { transform: `scale(1.015)${shake ? " translate(6px, -4px)" : ""}`, filter: type === 2 ? "url(#icx-chrom)" : "none" },
          { transform: "none", filter: "none" },
        ],
        { duration: 100, easing: "linear" },
      );
    }, 60000 / bpm);
    return () => window.clearInterval(t);
  }, [row?.id, type, bpm]);

  return (
    <div className={`icx-list${leaving ? " leaving" : ""}`} onContextMenu={(e) => (e.preventDefault(), back())}>
      <div className="icx-cam" ref={camRef}>
        {/* eslint-disable-next-line @next/next/no-img-element */}
        <img src={a("ic-bg.png")} alt="" className="icx-full" />
        {rows.map((r, i) => {
          const icon = icons[r.icon];
          const off = ICON_OFFSET[r.icon] ?? [0, 0];
          return (
            <div
              key={r.id}
              ref={(el) => {
                rowRefs.current[i] = el;
              }}
              className={`icx-row${i === sel ? " on" : ""}`}
              onClick={() => i !== sel && change(i - sel)}
              onWheel={(e) => change(e.deltaY > 0 ? 1 : -1)}
            >
              <RowContent text={songTitle(r.id)} letters={letters} a={a} icon={icon} off={off} />
            </div>
          );
        })}
        {row?.id === "bad-to-the-bone" && (
          // eslint-disable-next-line @next/next/no-img-element
          <img src={a("ic-jbug.png")} alt="" className="icx-jbug" />
        )}
      </div>

      <div className="icx-hud">
        <div className="icx-scorebg" />
        <p className="icx-score">PERSONAL BEST: {entry?.best.score ?? 0}</p>
        <p className={`icx-diff${locked ? " red" : ""}`}>{diffName}</p>
        {entry && entry.best.misses === 0 && <p className="icx-combo">FC</p>}
        <div className="icx-mechbg" />
        <p className="icx-mechinfo">Record di Relay</p>
        <p className="icx-mech">
          {entry ? (
            <>
              <span>{entry.best.accuracy != null ? `${(entry.best.accuracy * 100).toFixed(2)}%` : "—"}</span>
              <span>
                {entry.best.misses} miss · {formatDay(entry.best.recorded_at)}
              </span>
            </>
          ) : (
            <span className="none">Nessun record</span>
          )}
        </p>
        {clip && (
          <div className="icx-record">
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
              <div className="icx-archive">
                {[entry.best, ...entry.archive].map((c, i) => (
                  <button key={c.id} type="button" className={c.id === clip.id ? "on" : ""} onClick={() => setWatching(c.id)}>
                    {i === 0 ? "RECORD" : `#${i}`} {formatScore(c.score)}
                  </button>
                ))}
              </div>
            )}
          </div>
        )}
        <button type="button" className="icx-back" onClick={back} aria-label="Indietro">
          ←
        </button>
      </div>
      <svg width="0" height="0" aria-hidden="true" style={{ position: "absolute" }}>
        <filter id="icx-chrom">
          <feColorMatrix type="matrix" values="1 0 0 0 0  0 0 0 0 0  0 0 0 0 0  0 0 0 1 0" in="SourceGraphic" result="r" />
          <feOffset dx="6" in="r" result="r2" />
          <feColorMatrix type="matrix" values="0 0 0 0 0  0 1 0 0 0  0 0 1 0 0  0 0 0 1 0" in="SourceGraphic" result="gb" />
          <feOffset dx="-6" in="gb" result="gb2" />
          <feBlend in="r2" in2="gb2" mode="screen" />
        </filter>
      </svg>
    </div>
  );
}

function RowContent({ text, letters, a, icon, off }: { text: string; letters: Record<string, Strip>; a: (n: string) => string; icon?: Strip; off: [number, number] }) {
  const { width } = useWord(text, letters);
  return (
    <>
      <Word text={text} letters={letters} a={a} />
      {icon && <Anim s={icon} a={a} style={{ left: width + 10 - off[0], top: -30 - off[1] }} />}
    </>
  );
}

function Diamonds({ dir }: { dir: "out" | "in" }) {
  const ref = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const cv = ref.current;
    const ctx = cv?.getContext("2d");
    if (!cv || !ctx) return;
    const size = 30;
    const start = performance.now();
    let raf = 0;
    const draw = (now: number) => {
      const p = Math.min(1, (now - start) / 500);
      ctx.clearRect(0, 0, 1280, 720);
      ctx.fillStyle = "#000";
      for (let cy = 0; cy < 720; cy += size) {
        const v = (cy + size / 2) / 720;
        const r = p * 2 - v;
        for (let cx = 0; cx < 1280; cx += size) {
          if (dir === "out") {
            if (r <= 0) continue;
            if (r >= 1) ctx.fillRect(cx, cy, size, size);
            else diamond(ctx, cx, cy, size, r);
          } else {
            if (r >= 1) continue;
            if (r <= 0) ctx.fillRect(cx, cy, size, size);
            else {
              ctx.save();
              ctx.beginPath();
              ctx.rect(cx, cy, size, size);
              ctx.clip();
              ctx.beginPath();
              ctx.rect(cx, cy, size, size);
              diamondPath(ctx, cx, cy, size, r);
              ctx.fill("evenodd");
              ctx.restore();
            }
          }
        }
      }
      if (p < 1) raf = requestAnimationFrame(draw);
    };
    raf = requestAnimationFrame(draw);
    return () => cancelAnimationFrame(raf);
  }, [dir]);
  return <canvas ref={ref} width={1280} height={720} className="icx-diamonds" aria-hidden="true" />;
}

function diamondPath(ctx: CanvasRenderingContext2D, cx: number, cy: number, size: number, r: number) {
  const h = size / 2;
  const d = r * size;
  const mx = cx + h;
  const my = cy + h;
  ctx.moveTo(mx, my - d);
  ctx.lineTo(mx + d, my);
  ctx.lineTo(mx, my + d);
  ctx.lineTo(mx - d, my);
  ctx.closePath();
}

function diamond(ctx: CanvasRenderingContext2D, cx: number, cy: number, size: number, r: number) {
  ctx.save();
  ctx.beginPath();
  ctx.rect(cx, cy, size, size);
  ctx.clip();
  ctx.beginPath();
  diamondPath(ctx, cx, cy, size, r);
  ctx.fill();
  ctx.restore();
}
