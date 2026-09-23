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

  apiBase?: string;
  fnfTimeline?: { at_ms: number; song_name?: string; difficulty?: string; score?: number; accuracy?: number }[];
  fnfMisses?: { at_ms: number }[];
};

const TICK_MS = 250;

const NO_SIGNAL_MS = 20_000;

function PlayIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor" stroke="currentColor" strokeWidth="1.5" strokeLinejoin="round" aria-hidden="true">
      <path d="M8.5 5.7v12.6a1 1 0 0 0 1.5.87l10.9-6.3a1 1 0 0 0 0-1.74L10 4.83a1 1 0 0 0-1.5.87z" />
    </svg>
  );
}

function SpeakerIcon({ on }: { on: boolean }) {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M11 5 6 9H3v6h3l5 4z" fill="currentColor" strokeWidth="1.4" />
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
      <rect x="6" y="5" width="4" height="14" rx="1.6" />
      <rect x="14" y="5" width="4" height="14" rx="1.6" />
    </svg>
  );
}

function GearIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z" />
      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
    </svg>
  );
}

function CheckIcon() {
  return (
    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M5 13l5 5L19 7" />
    </svg>
  );
}

function FullscreenExitIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M9 3v3a2 2 0 0 1-2 2H4M15 3v3a2 2 0 0 0 2 2h3M9 21v-3a2 2 0 0 0-2-2H4M15 21v-3a2 2 0 0 1 2-2h3" />
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

export default function SyncPlayer({ matchId, players, live, names, mp4 = [], web = [], vod = [], apiBase, fnfTimeline = [], fnfMisses = [] }: Props) {
  const base = apiBase ?? `/api/matches/${encodeURIComponent(matchId)}`;
  const playersKey = players.join(",");
  const mp4Key = mp4.join(",");
  const webKey = web.join(",");
  const vodKey = vod.join(",");

  const [original, setOriginal] = useState(false);
  const [qualityMenuOpen, setQualityMenuOpen] = useState(false);

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

  const [chromeVisible, setChromeVisible] = useState(true);
  const hideTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

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
      const playerBase = `${base}/players/${encodeURIComponent(p)}`;
      if (mp4Key.split(",").includes(p)) {
        const light = !original && webKey.split(",").includes(p);
        const file = `${playerBase}/video.mp4${light ? "?q=web" : ""}`;
        if (light && vodKey.split(",").includes(p)) {
          const vodUrl = `${playerBase}/vod/index.m3u8`;
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
      const url = `${playerBase}/playlist.m3u8`;
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
  }, [matchId, base, playersKey, mp4Key, webKey, vodKey, original]);

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

  const focusedRef = useRef<string | null>(null);
  useEffect(() => {
    focusedRef.current = focused;
  }, [focused]);

  const bumpChrome = useCallback(() => {
    setChromeVisible(true);
    if (hideTimer.current) clearTimeout(hideTimer.current);
    if (focusedRef.current && wantPlay.current) {
      hideTimer.current = setTimeout(() => setChromeVisible(false), 4000);
    }
  }, []);

  const holdChrome = useCallback(() => {
    if (hideTimer.current) clearTimeout(hideTimer.current);
    setChromeVisible(true);
  }, []);

  useEffect(() => () => {
    if (hideTimer.current) clearTimeout(hideTimer.current);
  }, []);

  const togglePlay = useCallback(() => {
    wantPlay.current = !wantPlay.current;
    setPlaying(wantPlay.current);
    bumpChrome();
  }, [bumpChrome]);

  const lastVolume = useRef(1);
  const toggleMute = useCallback(() => {
    setVolume((v) => {
      if (v > 0) {
        lastVolume.current = v;
        return 0;
      }
      return lastVolume.current || 1;
    });
  }, [setVolume]);

  const exitFullscreen = useCallback(() => {
    if (document.fullscreenElement) void document.exitFullscreen();
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
    focusedRef.current = p;
    bumpChrome();
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
      if (!document.fullscreenElement) {
        setFocused(null);
        setChromeVisible(true);
      }
    };
    document.addEventListener("fullscreenchange", onChange);
    return () => document.removeEventListener("fullscreenchange", onChange);
  }, []);

  useEffect(() => {
    if (!qualityMenuOpen) return;
    const close = () => setQualityMenuOpen(false);
    const onEsc = (e: KeyboardEvent) => e.key === "Escape" && close();
    window.addEventListener("click", close);
    window.addEventListener("keydown", onEsc);
    return () => {
      window.removeEventListener("click", close);
      window.removeEventListener("keydown", onEsc);
    };
  }, [qualityMenuOpen]);

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
  const videoMs = current * 1000;
  const fnfNow = [...fnfTimeline].reverse().find((sample) => sample.at_ms <= videoMs);
  const missesNow = fnfMisses.filter((miss) => miss.at_ms <= videoMs).length;

  return (
    <div
      ref={containerRef}
      className={`player${focused ? " focused" : ""}${!chromeVisible ? " chrome-hidden" : ""}`}
      onMouseMove={bumpChrome}
      onTouchStart={bumpChrome}
    >
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

      {fnfNow && (
        <div className="fnfoverlay" aria-live="off">
          <strong>{fnfNow.song_name ?? "FNF"}</strong>
          {fnfNow.difficulty && <span>{fnfNow.difficulty}</span>}
          {fnfNow.score != null && <span>Score {fnfNow.score.toLocaleString("it-IT")}</span>}
          {fnfNow.accuracy != null && <span>{(fnfNow.accuracy * 100).toFixed(1)}%</span>}
          <span>{missesNow} miss</span>
        </div>
      )}

      <div className="controls" onMouseEnter={holdChrome} onMouseLeave={bumpChrome}>
        {qualityMenuOpen && (
          <div className="qualitymenu" onClick={(e) => e.stopPropagation()}>
            <div className="qualitymenu-title">Qualità</div>
            <button
              type="button"
              className={`qualityrow${!original ? " on" : ""}`}
              onClick={() => {
                setOriginal(false);
                setQualityMenuOpen(false);
              }}
            >
              <span className="qualitycheck">{!original && <CheckIcon />}</span>
              HD 720p
            </button>
            <button
              type="button"
              className={`qualityrow${original ? " on" : ""}`}
              onClick={() => {
                setOriginal(true);
                setQualityMenuOpen(false);
              }}
            >
              <span className="qualitycheck">{original && <CheckIcon />}</span>
              Originale
            </button>
          </div>
        )}
        <input
          type="range"
          className="seek"
          min={0}
          max={Math.max(end, 0.1)}
          step={0.1}
          value={Math.min(current, Math.max(end, 0.1))}
          aria-label="Posizione"
          style={{ ["--fill" as string]: `${end > 0 ? Math.min(100, (Math.min(current, end) / end) * 100) : 0}%` }}
          onChange={(e) => {
            dragging.current = true;
            setCurrent(Number(e.target.value));
          }}
          onPointerUp={(e) => {
            dragging.current = false;
            seekAll(Number((e.target as HTMLInputElement).value));
          }}
        />
        <div className="controls-row">
          <div className="controls-left">
            <button type="button" onClick={togglePlay} aria-label={playing ? "Pausa" : "Play"} className="cbtn">
              {playing ? <PauseIcon /> : <PlayIcon />}
            </button>
            <button type="button" onClick={toggleMute} aria-label={volume > 0 ? "Silenzia" : "Riattiva audio"} className="cbtn">
              <SpeakerIcon on={volume > 0} />
            </button>
            <input
              type="range"
              className="vol"
              min={0}
              max={1}
              step={0.05}
              value={volume}
              aria-label="Volume"
              title="Volume"
              style={{ ["--fill" as string]: `${volume * 100}%` }}
              onChange={(e) => setVolume(Number(e.target.value))}
            />
            <span className="time">{formatTime(current)} / {formatTime(end)}</span>
          </div>
          <div className="controls-right">
            {live && (
              <button type="button" onClick={goLive} className={`livebtn${behind < 15 ? " on" : ""}`}>
                Diretta{behind >= 15 ? ` (−${formatTime(behind)})` : ""}
              </button>
            )}
            {web.length > 0 && (
              <button
                type="button"
                className="cbtn"
                aria-label="Qualità del video"
                aria-expanded={qualityMenuOpen}
                title={original ? "Stai guardando la qualità originale" : "Stai guardando la versione leggera (720p)"}
                onClick={(e) => {
                  e.stopPropagation();
                  setQualityMenuOpen((v) => !v);
                }}
              >
                <GearIcon />
              </button>
            )}
            {focused && (
              <button type="button" onClick={exitFullscreen} aria-label="Esci da schermo intero" className="cbtn">
                <FullscreenExitIcon />
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
