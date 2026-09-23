"use client";

import { useEffect, useRef, useState } from "react";
import { isLive } from "@/lib/live";
import { formatBytes, stageLabel, statusOf, steamLibraryCover } from "@/lib/matchFormat";
import type { MatchDetail } from "@/lib/types";
import Avatar from "./Avatar";
import SyncPlayer from "./SyncPlayer";
import When from "./When";

const POLL_MS = 3000;

export default function ShareView({ initial, token, nowMs }: { initial: MatchDetail; token: string; nowMs: number }) {
  const [m, setM] = useState(initial);
  const [quality, setQuality] = useState<Record<string, "orig" | "web">>({});
  const base = `/api/share/${encodeURIComponent(token)}`;

  useEffect(() => {
    let stop = false;
    const load = async () => {
      try {
        const r = await fetch(base, { cache: "no-store" });
        if (r.ok && !stop) setM((await r.json()) as MatchDetail);
      } catch {
      }
    };
    const id = setInterval(load, POLL_MS);
    return () => {
      stop = true;
      clearInterval(id);
    };
  }, [base]);

  const label = (id: string) => m.names[id] ?? id;
  const st = statusOf(m, nowMs);
  const started = m.started_at_ms !== null;
  const live = isLive(m, nowMs);
  const title = m.name ?? (m.players.length ? m.players.map(label).join(", ") : "Partita");

  const qualityOf = (p: string): "orig" | "web" =>
    quality[p] ?? ((m.web_videos ?? []).includes(p) ? "web" : "orig");

  const [sizes, setSizes] = useState<Record<string, number | null>>({});
  const fetchSize = (p: string, q: "orig" | "web") => {
    const key = `${p}:${q}`;
    if (key in sizes) return;
    void fetch(`${base}/players/${encodeURIComponent(p)}/video.mp4${q === "web" ? "?q=web" : ""}`, { method: "HEAD" })
      .then((r) => {
        const len = r.headers.get("content-length");
        setSizes((prev) => ({ ...prev, [key]: len ? Number(len) : null }));
      })
      .catch(() => setSizes((prev) => ({ ...prev, [key]: null })));
  };
  const dlDialogRef = useRef<HTMLDialogElement>(null);
  const openDownloads = () => {
    for (const p of m.videos ?? []) fetchSize(p, qualityOf(p));
    dlDialogRef.current?.showModal();
  };

  return (
    <div className="matchpage">
      {m.game_name && (
        <div className="gamehero">
          {(steamLibraryCover(m.game_app_id) ?? m.game_cover_url) && (
            // eslint-disable-next-line @next/next/no-img-element
            <img src={steamLibraryCover(m.game_app_id) ?? m.game_cover_url} alt={`Copertina di ${m.game_name}`} />
          )}
          <div><span>Gioco</span><strong>{m.game_name}</strong></div>
        </div>
      )}
      {m.fnf_song_name && (
        <div className="fnfstats">
          <div><span>Canzone</span><strong>{m.fnf_song_name}</strong></div>
          {m.fnf_difficulty && <div><span>Difficolt&agrave;</span><strong>{m.fnf_difficulty}</strong></div>}
          {m.fnf_score != null && <div><span>Punteggio</span><strong>{m.fnf_score.toLocaleString("it-IT")}</strong></div>}
          {m.fnf_accuracy != null && <div><span>Accuracy</span><strong>{(m.fnf_accuracy * 100).toFixed(1)}%</strong></div>}
          <div><span>Note mancate</span><strong>{m.fnf_misses?.length ?? 0}</strong></div>
        </div>
      )}
      <div className="matchhead">
        <div>
          <h1>{title}</h1>
          <div className="meta">
            <When seconds={m.created_at} />
            <span className={`pill ${st.cls}`}>{st.label}</span>
            <span className="pill">Link privato condiviso</span>
          </div>
        </div>

        {(m.videos ?? []).length > 0 && (
          <button type="button" className="dlbtn" onClick={openDownloads}>
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <path d="M12 4v11m0 0-4-4m4 4 4-4M5 20h14" />
            </svg>
            Scarica
          </button>
        )}
      </div>

      <dialog
        ref={dlDialogRef}
        className="dldialog"
        onClick={(e) => {
          if (e.target === e.currentTarget) dlDialogRef.current?.close();
        }}
      >
        <div className="dldialog-head">
          <div>
            <h2>Scarica i video</h2>
            <p className="muted">{title}</p>
          </div>
          <button type="button" className="iconbtn" aria-label="Chiudi" onClick={() => dlDialogRef.current?.close()}>
            <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <path d="M18 6 6 18M6 6l12 12" />
            </svg>
          </button>
        </div>

        <div className="dllist">
          {(m.videos ?? []).map((p) => {
            const hasWeb = (m.web_videos ?? []).includes(p);
            const q = qualityOf(p);
            const size = sizes[`${p}:${q}`];
            return (
              <div key={p} className="dlrow">
                <Avatar name={label(p)} size={34} />
                <div className="dlinfo">
                  <span className="dlname">{label(p)}</span>
                  <span className="dlsize">{size ? formatBytes(size) : " "}</span>
                </div>
                {hasWeb && (
                  <div className="segmented" role="radiogroup" aria-label={`Qualità per ${label(p)}`}>
                    <button
                      type="button"
                      role="radio"
                      aria-checked={q === "web"}
                      className={q === "web" ? "on" : ""}
                      onClick={() => {
                        setQuality((prev) => ({ ...prev, [p]: "web" }));
                        fetchSize(p, "web");
                      }}
                    >
                      HD 720p
                    </button>
                    <button
                      type="button"
                      role="radio"
                      aria-checked={q === "orig"}
                      className={q === "orig" ? "on" : ""}
                      onClick={() => {
                        setQuality((prev) => ({ ...prev, [p]: "orig" }));
                        fetchSize(p, "orig");
                      }}
                    >
                      Originale
                    </button>
                  </div>
                )}
                <a
                  className="dlgo"
                  href={`${base}/players/${encodeURIComponent(p)}/video.mp4?download=1${q === "web" ? "&q=web" : ""}`}
                  download
                  title={`Scarica il video di ${label(p)} (${q === "web" ? "720p" : "originale"})`}
                  aria-label={`Scarica il video di ${label(p)} in ${q === "web" ? "720p" : "qualità originale"}`}
                >
                  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                    <path d="M12 4v11m0 0-4-4m4 4 4-4M5 20h14" />
                  </svg>
                </a>
              </div>
            );
          })}
        </div>
      </dialog>

      {m.players.some((p) => m.processing?.[p]) && (
        <div className="procbox" role="status" aria-live="polite">
          <div className="proctitle">Sto preparando i video della partita</div>
          {m.players
            .filter((p) => m.processing?.[p])
            .map((p) => {
              const pr = m.processing![p];
              return (
                <div key={p} className="procrow">
                  <span className="procname">{label(p)}</span>
                  <div
                    className="procbar"
                    role="progressbar"
                    aria-valuemin={0}
                    aria-valuemax={100}
                    aria-valuenow={pr.pct}
                    aria-label={`Video di ${label(p)}`}
                  >
                    <i style={{ width: `${pr.stage === "queue" ? 0 : pr.pct}%` }} />
                  </div>
                  <span className="procpct">{stageLabel(pr.stage, pr.pct)}</span>
                </div>
              );
            })}
        </div>
      )}

      {started ? (
        <SyncPlayer
          key={`${(m.videos ?? []).join(",")}:${(m.web_videos ?? []).join(",")}:${(m.vod_videos ?? []).join(",")}`}
          matchId={m.id}
          apiBase={base}
          players={m.players}
          names={m.names}
          live={live}
          mp4={m.videos ?? []}
          web={m.web_videos ?? []}
          vod={m.vod_videos ?? []}
        />
      ) : (
        <div className="emptystate">
          <span className="bigdot" aria-hidden="true" />
          <p>In attesa dell&apos;inizio</p>
        </div>
      )}
    </div>
  );
}
