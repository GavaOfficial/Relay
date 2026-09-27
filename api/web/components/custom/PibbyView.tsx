"use client";

import Link from "next/link";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { clipThumb, clipVideo, formatAccuracy, formatDay, formatScore, type Engine } from "@/lib/codename";
import { customAsset } from "@/lib/custom-assets";
import type { CodenameClip, CodenameModDetail, CodenameSong, FunkinTrack } from "@/lib/types";

const ARTISTS: Record<string, string> = {
  mindless: "Sevc_Ext_277",
  "blessed-by-swords": "Lettush",
  "brotherly-love": "Kylevi",
  "suffering-siblings": "Awe (ft. Saster)",
  "come-along-with-me": "Awe",
  "childs-play": "Yoosuf Meekail",
  "my-amazing-world": "Corn",
  retcon: "Rareblin (ft. Pattydecaffy)",
  "forgotten-world": "Awe",
};

function useTyped(text: string, speed: number, start: boolean) {
  const [n, setN] = useState(0);
  useEffect(() => {
    if (!start) return;
    let i = 0;
    const t = window.setInterval(() => {
      i += 1;
      setN(i);
      if (i >= text.length) window.clearInterval(t);
    }, speed);
    return () => {
      window.clearInterval(t);
      setN(0);
    };
  }, [text, speed, start]);
  return { shown: start ? text.slice(0, n) : "", done: start && n >= text.length };
}

export default function PibbyView({ initial, engine, fontClass }: { initial: CodenameModDetail; engine: Engine; fontClass: string }) {
  const mod = initial.mod;
  const catalog = mod.catalog;
  const extra = (catalog?.extra ?? {}) as { logo?: string; ui?: { backgrounds?: string[] } };
  const asset = useCallback((name?: string | null) => (name ? customAsset("pibby-apocalypse", name) : undefined), []);
  const songs: FunkinTrack[] = useMemo(() => catalog?.tracks ?? [], [catalog]);
  const backgrounds = extra.ui?.backgrounds ?? [];

  const recordsFor = useCallback(
    (t: FunkinTrack): CodenameSong[] => initial.songs.filter((e) => (e.song_id ?? e.song).toLowerCase() === t.id),
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

  const [bgFrame, setBgFrame] = useState(0);
  useEffect(() => {
    if (backgrounds.length < 2 || window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    const t = window.setInterval(() => setBgFrame((f) => (f + 1) % backgrounds.length), 100);
    return () => window.clearInterval(t);
  }, [backgrounds.length]);

  const [sel, setSel] = useState(() => Math.max(0, songs.findIndex((t) => recordsFor(t).length > 0)));
  const [watching, setWatching] = useState<string | null>(null);
  const [flash, setFlash] = useState(0);
  const flashTimer = useRef<number | undefined>(undefined);
  const move = useCallback(
    (d: number) => {
      if (!songs.length) return;
      setSel((v) => (v + d + songs.length) % songs.length);
      setWatching(null);
      setFlash(d);
      window.clearTimeout(flashTimer.current);
      flashTimer.current = window.setTimeout(() => setFlash(0), 200);
    },
    [songs.length],
  );
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.target instanceof HTMLElement && ["INPUT", "TEXTAREA", "VIDEO"].includes(e.target.tagName)) return;
      if (e.key === "ArrowLeft") move(-1);
      else if (e.key === "ArrowRight") move(1);
      else return;
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [move]);

  const track = songs[sel];
  const prev = songs.length > 1 ? songs[(sel - 1 + songs.length) % songs.length] : undefined;
  const next = songs.length > 1 ? songs[(sel + 1) % songs.length] : undefined;
  const entries = useMemo(() => (track ? recordsFor(track) : []), [track, recordsFor]);
  const best = entries.reduce<CodenameSong | null>((a, e) => (!a || e.best.score > a.best.score ? e : a), null);
  const clip: CodenameClip | undefined = best ? ([best.best, ...best.archive].find((c) => c.id === watching) ?? best.best) : undefined;
  const threat = Number((track?.extra as { threat?: number } | undefined)?.threat ?? 0);

  const title = useTyped((track?.name ?? "").toUpperCase(), 100, !!track);
  const artist = useTyped((ARTISTS[track?.id ?? ""] ?? "Kawai Sprite").toUpperCase(), 50, title.done);
  const stage = (t?: FunkinTrack) => (t ? asset(`pib-stage-${t.id}.png`) : undefined);
  const played = songs.filter((t) => recordsFor(t).length > 0).length;

  return (
    <div className={`impx pibx ${fontClass}`}>
      <div className="impx-navspace pibx-navspace" aria-hidden="true" />
      <div className="impx-corner">
        <Link href="/giochi/fnf" className="impx-close" aria-label="Chiudi e torna a Friday Night Funkin'">
          ✕
        </Link>
        {extra.logo && (
          // eslint-disable-next-line @next/next/no-img-element
          <img src={asset(extra.logo)} alt={catalog?.title ?? mod.name} className="impx-cornerlogo" />
        )}
      </div>

      <div className="mmx-stage pibx-stage" ref={stageRef}>
        {/* Ordine dei livelli come in create(): sfondo, threat, stage, riquadro, riquadri ai lati, frecce,
            testi, barra, vignettatura. */}
        <div className="mmx-screen pibx-screen" style={{ transform: `translate(-50%, -50%) scale(${scale})` }}>
          <div className="pibx-bg" aria-hidden="true">
            {backgrounds.map((f, i) => (
              // eslint-disable-next-line @next/next/no-img-element
              <img key={f} src={asset(f)} alt="" style={{ opacity: i === bgFrame ? 1 : 0 }} />
            ))}
          </div>
          {/* eslint-disable-next-line @next/next/no-img-element */}
          <img src={asset("pib-threat.png")} alt="" className="pibx-full" />

          <div className="pibx-center">
            {clip ? (
              <video
                key={clip.id}
                className="pibx-clip"
                src={clipVideo(clip.id, engine)}
                poster={clip.has_thumb ? clipThumb(clip.id, engine) : undefined}
                autoPlay
                muted
                loop
                playsInline
                controls
              />
            ) : (
              // eslint-disable-next-line @next/next/no-img-element
              <img key={track?.id} src={stage(track)} alt="" className="pibx-full pibx-stageimg" />
            )}
            {/* eslint-disable-next-line @next/next/no-img-element */}
            <img src={asset("pib-box.png")} alt="" className="pibx-full pibx-boxfront" />
          </div>

          {/* Nel gioco ai lati c'e' solo la cornice, vuota e trasparente: qui porta alla canzone accanto. */}
          {[
            { t: prev, side: "l", d: -1 },
            { t: next, side: "r", d: 1 },
          ].map(
            ({ t, side, d }) =>
              t && (
                <button key={side} type="button" className={`pibx-side ${side}`} onClick={() => move(d)} aria-label={t.name}>
                  {/* eslint-disable-next-line @next/next/no-img-element */}
                  <img src={asset("pib-box.png")} alt="" className="pibx-full" />
                </button>
              ),
          )}

          <button type="button" className={`pibx-arrow l${flash === -1 ? " flash" : ""}`} onClick={() => move(-1)} aria-label="Canzone precedente">
            {/* eslint-disable-next-line @next/next/no-img-element */}
            <img src={asset("pib-arrow-l.png")} alt="" />
          </button>
          <button type="button" className={`pibx-arrow r${flash === 1 ? " flash" : ""}`} onClick={() => move(1)} aria-label="Canzone successiva">
            {/* eslint-disable-next-line @next/next/no-img-element */}
            <img src={asset("pib-arrow-r.png")} alt="" />
          </button>

          <p className="pibx-title">{title.shown}</p>
          <p className="pibx-artist">{artist.shown}</p>

          <div className="pibx-bar" aria-label={`Threat level ${threat}%`}>
            {/* eslint-disable-next-line @next/next/no-img-element */}
            <img src={asset("pib-bar.png")} alt="" />
            <span style={{ width: `calc((100% - 8px) * ${threat / 100})` }} />
          </div>

          {/* eslint-disable-next-line @next/next/no-img-element */}
          <img src={asset("pib-gradient.png")} alt="" className="pibx-gradient" aria-hidden="true" />

          <div className="pibx-score">
            {best ? (
              <>
                <span>
                  PERSONAL BEST: {best.best.score} ({formatAccuracy(best.best.accuracy)})
                </span>
                <span>&lt; {best.difficulty.toUpperCase()} &gt;</span>
              </>
            ) : (
              <span>DA GIOCARE</span>
            )}
          </div>

          {best && (
            <div className="pibx-archive">
              <span>
                {best.best.misses} MISS · {formatDay(best.best.recorded_at).toUpperCase()}
              </span>
              {best.archive.length > 0 &&
                [best.best, ...best.archive].map((c, i) => (
                  <button key={c.id} type="button" className={c.id === clip?.id ? "on" : ""} onClick={() => setWatching(c.id)}>
                    {i === 0 ? "RECORD" : `#${i}`} {formatScore(c.score)}
                  </button>
                ))}
            </div>
          )}

          <p className="pibx-count">
            {sel + 1}/{songs.length} · {played} GIOCATE
          </p>
        </div>
      </div>
    </div>
  );
}
