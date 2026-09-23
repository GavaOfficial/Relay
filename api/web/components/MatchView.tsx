"use client";

import Link from "next/link";
import { useRouter } from "next/navigation";
import { useEffect, useRef, useState } from "react";
import { isLive } from "@/lib/live";
import { formatBytes, stageLabel, statusOf, steamLibraryCover } from "@/lib/matchFormat";
import type { MatchDetail } from "@/lib/types";
import Avatar from "./Avatar";
import SyncPlayer from "./SyncPlayer";
import When from "./When";

const POLL_MS = 3000;
type GameResult = { app_id?: number; name: string; cover_url?: string };

export default function MatchView({ initial, canRename = false, nowMs }: { initial: MatchDetail; canRename?: boolean; nowMs: number }) {
  const router = useRouter();
  const [m, setM] = useState(initial);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const [renameErr, setRenameErr] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [quality, setQuality] = useState<Record<string, "orig" | "web">>({});
  const [deletingMatch, setDeletingMatch] = useState(false);
  const [deleteErr, setDeleteErr] = useState<string | null>(null);
  const [gameEditing, setGameEditing] = useState(false);
  const [gameQuery, setGameQuery] = useState("");
  const [gameResults, setGameResults] = useState<GameResult[]>([]);
  const [gameBusy, setGameBusy] = useState(false);
  const [gameErr, setGameErr] = useState<string | null>(null);

  useEffect(() => {
    if (!gameEditing || gameQuery.trim().length < 2) {
      return;
    }
    const timer = setTimeout(async () => {
      try {
        const r = await fetch(`/api/games/search?q=${encodeURIComponent(gameQuery.trim())}`);
        if (r.ok) setGameResults((await r.json()) as GameResult[]);
      } catch {
        setGameResults([]);
      }
    }, 350);
    return () => clearTimeout(timer);
  }, [gameEditing, gameQuery]);

  const chooseGame = async (game: GameResult) => {
    setGameBusy(true);
    setGameErr(null);
    try {
      const r = await fetch(`/api/matches/${m.id}/game`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(game),
      });
      if (!r.ok) throw new Error(String(r.status));
      const info = (await r.json()) as MatchDetail;
      setM((prev) => ({ ...prev, game_app_id: info.game_app_id, game_name: info.game_name, game_cover_url: info.game_cover_url }));
      setGameEditing(false);
      setGameQuery("");
    } catch {
      setGameErr("Non sono riuscito a salvare il gioco. Riprova.");
    } finally {
      setGameBusy(false);
    }
  };

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

  const qualityOf = (p: string): "orig" | "web" =>
    quality[p] ?? ((m.web_videos ?? []).includes(p) ? "web" : "orig");

  const [sizes, setSizes] = useState<Record<string, number | null>>({});
  const fetchSize = (p: string, q: "orig" | "web") => {
    const key = `${p}:${q}`;
    if (key in sizes) return;
    void fetch(`/api/matches/${m.id}/players/${encodeURIComponent(p)}/video.mp4${q === "web" ? "?q=web" : ""}`, { method: "HEAD" })
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
  const downloadAll = () => {
    for (const p of m.videos ?? []) {
      const q = qualityOf(p);
      const a = document.createElement("a");
      a.href = `/api/matches/${m.id}/players/${encodeURIComponent(p)}/video.mp4?download=1${q === "web" ? "&q=web" : ""}`;
      a.download = "";
      document.body.appendChild(a);
      a.click();
      a.remove();
    }
  };

  const deleteMatch = async () => {
    if (!window.confirm("Eliminare l'intera partita e tutte le sue registrazioni? Non si può annullare.")) return;
    setDeletingMatch(true);
    setDeleteErr(null);
    try {
      const r = await fetch(`/api/matches/${m.id}`, { method: "DELETE" });
      if (!r.ok) throw new Error(String(r.status));
      router.push("/");
    } catch {
      setDeleteErr("Non sono riuscito a eliminare la partita. Riprova.");
      setDeletingMatch(false);
    }
  };

  const shareDialogRef = useRef<HTMLDialogElement>(null);
  const [shareBusy, setShareBusy] = useState(false);
  const [shareErr, setShareErr] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [origin, setOrigin] = useState("");
  const shareUrl = m.share_token ? `${origin}/s/${m.share_token}` : "";

  const openShare = () => {
    setShareErr(null);
    setOrigin(window.location.origin);
    shareDialogRef.current?.showModal();
  };
  const enableShare = async () => {
    setShareBusy(true);
    setShareErr(null);
    try {
      const r = await fetch(`/api/matches/${m.id}/share`, { method: "POST" });
      if (!r.ok) throw new Error(String(r.status));
      const info = (await r.json()) as MatchDetail;
      setM((prev) => ({ ...prev, share_token: info.share_token }));
    } catch {
      setShareErr("Non sono riuscito ad attivare la condivisione. Riprova.");
    } finally {
      setShareBusy(false);
    }
  };
  const disableShare = async () => {
    setShareBusy(true);
    setShareErr(null);
    try {
      const r = await fetch(`/api/matches/${m.id}/share`, { method: "DELETE" });
      if (!r.ok) throw new Error(String(r.status));
      setM((prev) => ({ ...prev, share_token: undefined }));
    } catch {
      setShareErr("Non sono riuscito a disattivare la condivisione. Riprova.");
    } finally {
      setShareBusy(false);
    }
  };
  const copyShare = async () => {
    try {
      await navigator.clipboard.writeText(shareUrl);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      setShareErr("Non sono riuscito a copiare il link.");
    }
  };

  return (
    <div className="matchpage">
      <Link href="/" className="back">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
          <path d="m15 18-6-6 6-6" />
        </svg>
        Partite
      </Link>

      {m.game_name && (
        <div className="gamehero">
          {(steamLibraryCover(m.game_app_id) ?? m.game_cover_url) && (
            // eslint-disable-next-line @next/next/no-img-element
            <img src={steamLibraryCover(m.game_app_id) ?? m.game_cover_url} alt={`Copertina di ${m.game_name}`} />
          )}
          <div><span>Gioco</span><strong>{m.game_name}</strong></div>
          {canRename && <button type="button" className="ghost small gameeditbtn" onClick={() => { setGameResults([]); setGameEditing((value) => !value); }}>Cambia</button>}
        </div>
      )}
      {canRename && !m.game_name && <button type="button" className="ghost gameaddbtn" onClick={() => { setGameResults([]); setGameEditing((value) => !value); }}>Aggiungi gioco e copertina</button>}
      {canRename && gameEditing && (
        <div className="gamepicker">
          <input autoFocus type="search" value={gameQuery} placeholder="Cerca un gioco..." onChange={(e) => setGameQuery(e.target.value)} />
          {gameResults.map((game) => (
            <button type="button" key={`${game.app_id ?? "game"}-${game.name}`} disabled={gameBusy} onClick={() => void chooseGame(game)}>
              {(steamLibraryCover(game.app_id) ?? game.cover_url) && (
                // eslint-disable-next-line @next/next/no-img-element
                <img src={steamLibraryCover(game.app_id) ?? game.cover_url} alt="" />
              )}
              <span>{game.name}</span>
            </button>
          ))}
          {gameQuery.trim().length >= 2 && !gameResults.length && <p className="muted">Nessun risultato.</p>}
          {gameErr && <p className="error">{gameErr}</p>}
        </div>
      )}

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
              {canRename && m.status === "ended" && (
                <button
                  type="button"
                  className="iconbtn danger"
                  title="Elimina la partita"
                  aria-label="Elimina la partita"
                  disabled={deletingMatch}
                  onClick={() => void deleteMatch()}
                >
                  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                    <path d="M3 6h18M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2m3 0-1 14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2L4 6h16ZM10 11v6M14 11v6" />
                  </svg>
                </button>
              )}
            </div>
          )}
          {renameErr && <p className="error">{renameErr}</p>}
          {deleteErr && <p className="error">{deleteErr}</p>}
          <div className="meta">
            <When seconds={m.created_at} />
            <span className={`pill ${st.cls}`}>{st.label}</span>
          </div>
        </div>

        <div className="matchhead-actions">
          {canRename && m.status === "ended" && (m.videos ?? []).length > 0 && (
            <button type="button" className="dlbtn" onClick={openShare}>
              <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                <circle cx="18" cy="5" r="3" />
                <circle cx="6" cy="12" r="3" />
                <circle cx="18" cy="19" r="3" />
                <path d="m8.6 10.5 6.8-3.9M8.6 13.5l6.8 3.9" />
              </svg>
              Condividi
            </button>
          )}
          {(m.videos ?? []).length > 0 && (
            <button type="button" className="dlbtn" onClick={openDownloads}>
              <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                <path d="M12 4v11m0 0-4-4m4 4 4-4M5 20h14" />
              </svg>
              Scarica
            </button>
          )}
        </div>
      </div>

      <dialog
        ref={shareDialogRef}
        className="dldialog sharedialog"
        onClick={(e) => {
          if (e.target === e.currentTarget) shareDialogRef.current?.close();
        }}
      >
        <div className="dldialog-head">
          <div>
            <h2>Condividi</h2>
            <p className="muted">{title}</p>
          </div>
          <button type="button" className="iconbtn" aria-label="Chiudi" onClick={() => shareDialogRef.current?.close()}>
            <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <path d="M18 6 6 18M6 6l12 12" />
            </svg>
          </button>
        </div>
        <div className="sharebody">
          {m.share_token ? (
            <>
              <p className="muted">
                Chiunque abbia questo link può vedere la partita, senza accedere e senza comparire nel sito.
              </p>
              <div className="sharelink">
                <input type="text" readOnly value={shareUrl} onFocus={(e) => e.currentTarget.select()} />
                <button type="button" className="ghost small" onClick={() => void copyShare()}>
                  {copied ? "Copiato!" : "Copia"}
                </button>
              </div>
              {shareErr && <p className="error">{shareErr}</p>}
              <button type="button" className="ghost" disabled={shareBusy} onClick={() => void disableShare()}>
                Disattiva la condivisione
              </button>
            </>
          ) : (
            <>
              <p className="muted">
                Crea un link privato per far vedere questa partita anche a chi non ha un account: non comparirà nel
                sito, ma chiunque abbia il link potrà aprirlo.
              </p>
              {shareErr && <p className="error">{shareErr}</p>}
              <button type="button" className="primary" disabled={shareBusy} onClick={() => void enableShare()}>
                Crea il link
              </button>
            </>
          )}
        </div>
      </dialog>

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
                  <span className="dlsize">{size ? formatBytes(size) : " "}</span>
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
                  href={`/api/matches/${m.id}/players/${encodeURIComponent(p)}/video.mp4?download=1${q === "web" ? "&q=web" : ""}`}
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

        {(m.videos ?? []).length > 1 && (
          <div className="dldialog-foot">
            <button type="button" className="ghost small" onClick={downloadAll}>
              Scarica tutti
            </button>
          </div>
        )}
      </dialog>

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

      {m.fnf_song_name && (
        <div className="fnfstats">
          <div><span>Canzone</span><strong>{m.fnf_song_name}</strong></div>
          {m.fnf_difficulty && <div><span>Difficolt&agrave;</span><strong>{m.fnf_difficulty}</strong></div>}
          {m.fnf_score != null && <div><span>Punteggio</span><strong>{m.fnf_score.toLocaleString("it-IT")}</strong></div>}
          {m.fnf_accuracy != null && <div><span>Accuracy</span><strong>{(m.fnf_accuracy * 100).toFixed(1)}%</strong></div>}
          <div><span>Note mancate</span><strong>{m.fnf_misses?.length ?? 0}</strong></div>
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
          fnfTimeline={m.fnf_timeline ?? []}
          fnfMisses={m.fnf_misses ?? []}
        />
      ) : (
        <div className="emptystate">
          <span className="bigdot" aria-hidden="true" />
          <p>In attesa dell&apos;inizio</p>
          <p className="muted">Le visuali compariranno qui appena l&apos;host avvia la registrazione dall&apos;app.</p>
        </div>
      )}
    </div>
  );
}
