"use client";

import Link from "next/link";
import { useEffect, useState } from "react";
import { isLive } from "@/lib/live";
import type { MatchDetail } from "@/lib/types";
import Avatar from "./Avatar";
import SyncPlayer from "./SyncPlayer";
import When from "./When";

const POLL_MS = 3000;

function statusOf(m: MatchDetail, nowMs: number): { label: string; cls: string } {
  if (m.status === "ended") return { label: "Terminata", cls: "" };
  if (m.stopped_at_ms) return { label: "Caricamento", cls: "" };
  if (m.started_at_ms) return { label: isLive(m, nowMs) ? "In diretta" : "Registrata", cls: isLive(m, nowMs) ? "live" : "" };
  return { label: "In attesa", cls: "idle" };
}

function stageLabel(stage: "queue" | "video" | "web", pct: number): string {
  if (stage === "queue") return "In coda";
  if (stage === "web") return `Versione leggera ${pct}%`;
  return `Preparo il video ${pct}%`;
}

export default function MatchView({ initial, canRename = false, nowMs }: { initial: MatchDetail; canRename?: boolean; nowMs: number }) {
  const [m, setM] = useState(initial);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const [renameErr, setRenameErr] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let stop = false;
    const load = async () => {
      try {
        const r = await fetch(`/api/matches/${initial.id}`, { cache: "no-store" });
        if (r.ok && !stop) setM((await r.json()) as MatchDetail);
      } catch {
      }
    };
    const id = setInterval(load, POLL_MS);
    return () => {
      stop = true;
      clearInterval(id);
    };
  }, [initial.id]);

  const label = (id: string) => m.names[id] ?? id;
  const st = statusOf(m, nowMs);
  const started = m.started_at_ms !== null;

  const live = isLive(m, nowMs);
  const title = m.name ?? (m.players.length ? m.players.map(label).join(", ") : "Partita");

  const saveName = async () => {
    setSaving(true);
    setRenameErr(null);
    try {
      const r = await fetch(`/api/matches/${m.id}/rename`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ name: draft }),
      });
      if (!r.ok) throw new Error(String(r.status));
      const info = (await r.json()) as MatchDetail;
      setM((prev) => ({ ...prev, name: info.name }));
      setEditing(false);
    } catch {
      setRenameErr("Non sono riuscito a cambiare il nome. Riprova.");
    } finally {
      setSaving(false);
    }
  };

  return (
    <>
      <Link href="/" className="back">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
          <path d="m15 18-6-6 6-6" />
        </svg>
        Partite
      </Link>

      <div className="matchhead">
        <div>
          {editing ? (
            <form
              className="renameform"
              onSubmit={(e) => {
                e.preventDefault();
                void saveName();
              }}
            >
              <input
                autoFocus
                value={draft}
                maxLength={60}
                placeholder="Nome della partita"
                aria-label="Nome della partita"
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={(e) => e.key === "Escape" && setEditing(false)}
              />
              <button type="submit" className="primary small" disabled={saving}>
                Salva
              </button>
              <button type="button" className="ghost small" onClick={() => setEditing(false)}>
                Annulla
              </button>
            </form>
          ) : (
            <div className="titlerow">
              <h1>{title}</h1>
              {canRename && (
                <button
                  type="button"
                  className="iconbtn"
                  title="Rinomina la partita"
                  aria-label="Rinomina la partita"
                  onClick={() => {
                    setDraft(m.name ?? "");
                    setRenameErr(null);
                    setEditing(true);
                  }}
                >
                  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                    <path d="M12 20h9M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z" />
                  </svg>
                </button>
              )}
            </div>
          )}
          {renameErr && <p className="error">{renameErr}</p>}
          <div className="meta">
            <When seconds={m.created_at} />
            <span className={`pill ${st.cls}`}>{st.label}</span>
          </div>
        </div>

        {(m.videos ?? []).length > 0 && (
          <details className="dlmenu">
            <summary>
              <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                <path d="M12 4v11m0 0-4-4m4 4 4-4M5 20h14" />
              </svg>
              Scarica
            </summary>
            <div className="dlpop">
              {(m.videos ?? []).map((p) => (
                <a
                  key={p}
                  href={`/api/matches/${m.id}/players/${encodeURIComponent(p)}/video.mp4?download=1`}
                  download
                  title={`Scarica il video di ${label(p)}`}
                >
                  {label(p)}
                </a>
              ))}
            </div>
          </details>
        )}
      </div>

      {live && m.players.length > 0 && (
        <div className="players" aria-label="Giocatori">
          {m.players.map((p) => (
            <span key={p} className={`player-chip${m.connected.includes(p) ? " on" : ""}`}>
              <Avatar name={label(p)} size={22} />
              {label(p)}
              <span className="dot" aria-label={m.connected.includes(p) ? "connesso" : "non connesso"} />
            </span>
          ))}
        </div>
      )}

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
          <p className="prochint">Intanto puoi già guardare il replay: quando il video è pronto passa da solo alla versione finale.</p>
        </div>
      )}

      {started ? (
        <SyncPlayer
          key={`${m.id}:${m.players.join(",")}:${(m.videos ?? []).join(",")}:${(m.web_videos ?? []).join(",")}:${(m.vod_videos ?? []).join(",")}`}
          matchId={m.id}
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
          <p className="muted">Le visuali compariranno qui appena l&apos;host avvia la registrazione dall&apos;app.</p>
        </div>
      )}
    </>
  );
}
