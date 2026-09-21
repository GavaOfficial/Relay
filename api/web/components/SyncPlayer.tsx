"use client";

import Hls from "hls.js";
import { useCallback, useEffect, useRef, useState } from "react";
import { clamp, commonEnd, driftAction, formatTime, livePosition, longestEnd } from "@/lib/sync";

type Props = {
  matchId: string;
  players: string[];

  live: boolean;

  names?: Record<string, string>;

  mp4?: string[];

  web?: string[];

  vod?: string[];
};

const TICK_MS = 250;

const NO_SIGNAL_MS = 20_000;

function PlayIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
      <path d="M8 5.5v13a1 1 0 0 0 1.5.86l10.5-6.5a1 1 0 0 0 0-1.72L9.5 4.64A1 1 0 0 0 8 5.5z" />
    </svg>
  );
}

function SpeakerIcon({ on }: { on: boolean }) {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M11 5 6 9H3v6h3l5 4z" fill="currentColor" stroke="none" />
      {on ? (
        <>
          <path d="M15.5 8.5a5 5 0 0 1 0 7" />
          <path d="M18.5 5.5a9 9 0 0 1 0 13" />
        </>
      ) : (
        <path d="m16 9 5 6m0-6-5 6" />
      )}
    </svg>
  );
}

function PauseIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
      <rect x="6" y="5" width="4" height="14" rx="1.2" />
      <rect x="14" y="5" width="4" height="14" rx="1.2" />
    </svg>
  );
}

function seekableEnd(v: HTMLVideoElement): number | null {
  if (v.seekable.length > 0) return v.seekable.end(v.seekable.length - 1);
  return Number.isFinite(v.duration) ? v.duration : null;
}

function finished(v: HTMLVideoElement): boolean {
  const e = seekableEnd(v);
  return v.ended || (e !== null && v.currentTime >= e - 0.25);
}

export default function SyncPlayer({ matchId, players, live, names, mp4 = [], web = [], vod = [] }: Props) {
  const playersKey = players.join(",");
  const mp4Key = mp4.join(",");
  const webKey = web.join(",");
  const vodKey = vod.join(",");

  const [original, setOriginal] = useState(false);

  const containerRef = useRef<HTMLDivElement>(null);
  const videos = useRef<Record<string, HTMLVideoElement | null>>({});
  const dead = useRef<Set<string>>(new Set());
  const silentSince = useRef<Record<string, number>>({});
  const aligned = useRef(false);
  const dragging = useRef(false);
  const wantPlay = useRef(true);
  const liveRef = useRef(live);
  const aliveRef = useRef<HTMLVideoElement[]>([]);
  const masterRef = useRef<HTMLVideoElement | null>(null);

  const [playing, setPlaying] = useState(true);
  const [ready, setReady] = useState(false);
  const [buffering, setBuffering] = useState(false);
  const [current, setCurrent] = useState(0);
  const [end, setEnd] = useState(0);
  const [focused, setFocused] = useState<string | null>(null);
  const [noSignal, setNoSignal] = useState("");

  const [audioFrom, setAudioFrom] = useState<string | null>(null);
  const [volume, setVolume] = useState(1);
  const listening = focused ?? audioFrom;

  useEffect(() => {
    liveRef.current = live;
  }, [live]);

  useEffect(() => {
    const created: Hls[] = [];
    const timers: ReturnType<typeof setTimeout>[] = [];
    dead.current = new Set();
    silentSince.current = {};
    aligned.current = false;

    for (const p of playersKey.split(",")) {
      const v = videos.current[p];
      if (!v) continue;
      const base = `/api/matches/${encodeURIComponent(matchId)}/players/${encodeURIComponent(p)}`;
      if (mp4Key.split(",").includes(p)) {
        const light = !original && webKey.split(",").includes(p);
        const file = `${base}/video.mp4${light ? "?q=web" : ""}`;
        if (light && vodKey.split(",").includes(p)) {
          const vodUrl = `${base}/vod/index.m3u8`;
          if (Hls.isSupported()) {
            const hls = new Hls({ maxBufferLength: 30, maxMaxBufferLength: 60, backBufferLength: 30, maxBufferSize: 60 * 1000 * 1000 });
            created.push(hls);
            hls.on(Hls.Events.ERROR, (_e, data) => {
              if (!data.fatal) return;
              if (data.type === Hls.ErrorTypes.NETWORK_ERROR) {
                timers.push(setTimeout(() => hls.startLoad(), 2000));
              } else if (data.type === Hls.ErrorTypes.MEDIA_ERROR) {
                hls.recoverMediaError();
              } else {
                hls.destroy();
                v.src = file;
              }
            });
            hls.loadSource(vodUrl);
            hls.attachMedia(v);
            continue;
          }
          if (v.canPlayType("application/vnd.apple.mpegurl")) {
            v.src = vodUrl;
            continue;
          }
        }
        v.src = file;
        continue;
      }
      const url = `${base}/playlist.m3u8`;
      if (Hls.isSupported()) {
        const hls = new Hls({ backBufferLength: 90 });
        created.push(hls);
        hls.on(Hls.Events.ERROR, (_e, data) => {
          if (!data.fatal) return;
          if (data.type === Hls.ErrorTypes.NETWORK_ERROR) {
            timers.push(setTimeout(() => hls.startLoad(), 2000));
          } else if (data.type === Hls.ErrorTypes.MEDIA_ERROR) {
            hls.recoverMediaError();
          } else {
            dead.current.add(p);
          }
        });
        hls.loadSource(url);
        hls.attachMedia(v);
      } else if (v.canPlayType("application/vnd.apple.mpegurl")) {
        v.src = url;
      } else {
        dead.current.add(p);
      }
    }
    return () => {
      timers.forEach(clearTimeout);
      created.forEach((h) => h.destroy());
    };
  }, [matchId, playersKey, mp4Key, webKey, vodKey, original]);

  const seekAll = useCallback((t: number) => {
    const alive = aliveRef.current;

    const ends = alive.map(seekableEnd);
    const max = (liveRef.current ? commonEnd(ends) : longestEnd(ends)) ?? t;
    const target = clamp(t, 0, max);
    for (const v of alive) v.currentTime = target;
    setCurrent(target);
  }, [setCurrent]);

  useEffect(() => {
    const tick = () => {
      const now = Date.now();
      const alive: HTMLVideoElement[] = [];
      const silent: string[] = [];
      for (const p of playersKey.split(",")) {
        const v = videos.current[p];
        if (!v) continue;
        if (v.readyState >= 1) delete silentSince.current[p];
        else silentSince.current[p] ??= now;
        const offline =
          dead.current.has(p) || (silentSince.current[p] !== undefined && now - silentSince.current[p] > NO_SIGNAL_MS);
        if (offline) silent.push(p);
        else alive.push(v);
      }
      aliveRef.current = alive;
      setNoSignal((prev) => (prev === silent.join(",") ? prev : silent.join(",")));
      if (alive.length === 0) return;

      if (!aligned.current) {
        if (alive.every((v) => v.readyState >= 1 && v.seekable.length > 0)) {
          const e = commonEnd(alive.map(seekableEnd));
          const t = liveRef.current && e !== null ? livePosition(e) : 0;
          for (const v of alive) v.currentTime = t;
          aligned.current = true;
          setReady(true);
        }
        return;
      }

      const starved = alive.some((v) => v.readyState < 3 && !finished(v));
      setBuffering(wantPlay.current && starved);
      if (wantPlay.current && !starved) {
        for (const v of alive) {
          if (v.paused && !finished(v)) {
            v.play().catch(() => {
              wantPlay.current = false;
              setPlaying(false);
            });
          }
        }
      } else {
        for (const v of alive) if (!v.paused) v.pause();
      }

      let master = alive[0];
      if (!liveRef.current) {
        for (const v of alive) if ((seekableEnd(v) ?? 0) > (seekableEnd(master) ?? 0)) master = v;
      }
      masterRef.current = master;
      master.playbackRate = 1;
      for (const v of alive) {
        if (v === master) continue;
        if (v.seeking || master.seeking) continue;
        if (finished(v)) continue;
        if (!wantPlay.current || starved) {
          if (Math.abs(v.currentTime - master.currentTime) > 0.05) v.currentTime = master.currentTime;
          continue;
        }
        const a = driftAction(master.currentTime, v.currentTime);
        if (a.seekTo !== null) v.currentTime = a.seekTo;
        if (v.playbackRate !== a.rate) v.playbackRate = a.rate;
      }

      const ends = alive.map(seekableEnd);
      const e = (liveRef.current ? commonEnd(ends) : longestEnd(ends)) ?? 0;
      setEnd(e);
      if (!dragging.current) setCurrent(master.currentTime);
    };
    const id = setInterval(tick, TICK_MS);
    return () => clearInterval(id);
  }, [playersKey]);

  useEffect(() => {
    for (const p of playersKey.split(",")) {
      const v = videos.current[p];
      if (!v) continue;
      v.muted = p !== listening;
      v.volume = volume;
    }
  }, [listening, volume, playersKey, mp4Key, ready]);

  const togglePlay = useCallback(() => {
    wantPlay.current = !wantPlay.current;
    setPlaying(wantPlay.current);
  }, []);

  const goLive = useCallback(() => {
    const e = commonEnd(aliveRef.current.map(seekableEnd));
    if (e !== null) seekAll(livePosition(e, 6));
  }, [seekAll]);

  const onTileClick = (p: string) => {
    if (document.fullscreenElement) {
      void document.exitFullscreen();
      return;
    }
    setFocused(p);
    const c = containerRef.current;
    if (c && typeof c.requestFullscreen === "function") {
      c.requestFullscreen().catch(() => setFocused(null));
      return;
    }

    const v = videos.current[p] as (HTMLVideoElement & { webkitEnterFullscreen?: () => void }) | null | undefined;
    if (v?.webkitEnterFullscreen) v.webkitEnterFullscreen();
    else setFocused(null);
  };
  useEffect(() => {
    const onChange = () => {
      if (!document.fullscreenElement) setFocused(null);
    };
    document.addEventListener("fullscreenchange", onChange);
    return () => document.removeEventListener("fullscreenchange", onChange);
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      if (t && ["INPUT", "TEXTAREA", "BUTTON"].includes(t.tagName) && e.key === " ") return;
      if (e.key === " ") {
        e.preventDefault();
        togglePlay();
      } else if (e.key === "ArrowRight" || e.key === "ArrowLeft") {
        e.preventDefault();
        const master = masterRef.current ?? aliveRef.current[0];
        if (master) seekAll(master.currentTime + (e.key === "ArrowRight" ? 5 : -5));
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [togglePlay, seekAll]);

  const behind = Math.max(0, end - current);
  const silentList = noSignal ? noSignal.split(",") : [];

  return (
    <div ref={containerRef} className={`player${focused ? " focused" : ""}`}>
      <div className="tiles">
        {players.map((p) => (
          <figure
            key={p}
            className={`tile${focused && focused !== p ? " off" : ""}`}
            onClick={() => onTileClick(p)}
            title="Clic per lo schermo intero"
          >
            <video
              ref={(el) => {
                videos.current[p] = el;
              }}
              muted
              playsInline
              preload="metadata"
            />
            <figcaption>{names?.[p] ?? p}</figcaption>
            <button
              type="button"
              className={`audiobtn${listening === p ? " on" : ""}`}
              aria-pressed={listening === p}
              aria-label={listening === p ? `Silenzia ${names?.[p] ?? p}` : `Ascolta ${names?.[p] ?? p}`}
              title={listening === p ? "Stai ascoltando questa visuale" : "Ascolta questa visuale"}
              onClick={(e) => {
                e.stopPropagation();
                setAudioFrom((prev) => (prev === p ? null : p));
              }}
            >
              <SpeakerIcon on={listening === p} />
            </button>
            {silentList.includes(p) && <div className="overlay">Nessun segnale</div>}
          </figure>
        ))}
        {!ready && <div className="overlay center">Carico i video…</div>}
        {ready && buffering && <div className="overlay center">Buffering…</div>}
      </div>

      <div className="controls">
        <button type="button" onClick={togglePlay} aria-label={playing ? "Pausa" : "Play"} className="icon">
          {playing ? <PauseIcon /> : <PlayIcon />}
        </button>
        <span className="time">{formatTime(current)}</span>
        <input
          type="range"
          min={0}
          max={Math.max(end, 0.1)}
          step={0.1}
          value={Math.min(current, Math.max(end, 0.1))}
          aria-label="Posizione"
          onChange={(e) => {
            dragging.current = true;
            setCurrent(Number(e.target.value));
          }}
          onPointerUp={(e) => {
            dragging.current = false;
            seekAll(Number((e.target as HTMLInputElement).value));
          }}
        />
        <span className="time">{formatTime(end)}</span>
        {web.length > 0 && (
          <button
            type="button"
            className={`qbtn${original ? " on" : ""}`}
            aria-pressed={original}
            title={original ? "Stai guardando la qualità originale" : "Stai guardando la versione leggera (720p): parte prima e pesa meno"}
            onClick={() => setOriginal((v) => !v)}
          >
            {original ? "Originale" : "720p"}
          </button>
        )}
        <input
          type="range"
          className="vol"
          min={0}
          max={1}
          step={0.05}
          value={volume}
          aria-label="Volume"
          title="Volume"
          onChange={(e) => setVolume(Number(e.target.value))}
        />
        {live && (
          <button type="button" onClick={goLive} className={`livebtn${behind < 15 ? " on" : ""}`}>
            Diretta{behind >= 15 ? ` (−${formatTime(behind)})` : ""}
          </button>
        )}
      </div>
    </div>
  );
}
