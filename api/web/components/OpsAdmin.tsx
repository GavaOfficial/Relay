"use client";

import { useCallback, useEffect, useState } from "react";

export type OpsNode = {
  id: string;
  name: string;
  created_at: number;
  online: boolean;
  version: string | null;
  threads: number;
  load: number;
  ffmpeg: string | null;
  jobs: { label: string; stage: string; pct: number }[];
  parallel: number | null;
  limit: number;
  slots: number;
};

export type OpsJob = {
  id: string;
  label: string;
  created_at: number;
  attempts: number;
  last_error: string | null;
  running: boolean;
  retry_in: number;
};

export type OpsStatus = { nodes: OpsNode[]; queue: OpsJob[] };

async function call(path: string, method: string, body?: unknown) {
  const r = await fetch(path, {
    method,
    headers: body === undefined ? undefined : { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!r.ok) throw new Error((await r.text()) || `errore ${r.status}`);
  return r.status === 204 || r.status === 202 ? null : r.json();
}

function installCommand(key: string) {
  const origin = typeof window === "undefined" ? "https://relay.gavatech.org" : window.location.origin;
  return `curl -fsSL ${origin}/api/ops/install.sh | sudo bash -s -- --key ${key} --server ${origin}`;
}

const STAGES: Record<string, string> = {
  download: "Scarico i pezzi",
  video: "Unisco il video",
  web: "Versione leggera",
  upload: "Invio all'archivio",
};

function waitText(secs: number) {
  if (secs <= 0) return "";
  const min = Math.ceil(secs / 60);
  return ` · riprovo tra ${min} min`;
}

export default function OpsAdmin({ initial }: { initial: OpsStatus }) {
  const [st, setSt] = useState(initial);
  const [err, setErr] = useState<string | null>(null);
  const [newName, setNewName] = useState("");
  const [shownKey, setShownKey] = useState<{ name: string; key: string } | null>(null);

  const refresh = useCallback(async () => {
    try {
      setSt((await call("/api/ops/status", "GET")) as OpsStatus);
    } catch {
      /* resta l'ultimo stato */
    }
  }, []);
  useEffect(() => {
    const t = window.setInterval(refresh, 3000);
    return () => window.clearInterval(t);
  }, [refresh]);

  const act = async (f: () => Promise<unknown>) => {
    setErr(null);
    try {
      await f();
      await refresh();
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    }
  };

  const waiting = st.queue.filter((j) => !j.running);
  return (
    <div className="stor">
      <div className="stor-section">
        <h2>Server operazioni</h2>
        <p className="muted">
          Uniscono i pezzi delle registrazioni, creano la versione leggera e tagliano le clip con ffmpeg. Se ce n&apos;è almeno uno,
          il server centrale non usa più ffmpeg e i lavori aspettano che un server operazioni sia collegato.
        </p>
      </div>

      {st.nodes.map((n) => (
        <OpsCard key={n.id} n={n} act={act} onKey={(key) => setShownKey({ name: n.name, key })} />
      ))}

      {shownKey && (
        <section className="stor-card stor-key" role="alert">
          <h2>Chiave di «{shownKey.name}»</h2>
          <p className="muted">
            Si vede solo adesso: copiala. Sul PC Ubuntu esegui questo comando (installa ffmpeg e relay-ops come servizio e lo collega):
          </p>
          <pre className="stor-cmd">{installCommand(shownKey.key)}</pre>
          <div className="stor-row">
            <button type="button" onClick={() => void navigator.clipboard?.writeText(installCommand(shownKey.key))}>
              Copia il comando
            </button>
            <button type="button" className="ghost" onClick={() => setShownKey(null)}>
              Fatto
            </button>
          </div>
        </section>
      )}

      <section className="stor-card">
        <div className="stor-head">
          <h2>Lavori in coda</h2>
          {waiting.some((j) => j.retry_in > 0) && (
            <button type="button" className="ghost small" onClick={() => void act(() => call("/api/ops/retry", "POST"))}>
              Riprova ora
            </button>
          )}
        </div>
        {st.queue.length === 0 ? (
          <p className="muted">Nessun lavoro in attesa.</p>
        ) : (
          <ul className="ops-queue">
            {st.queue.map((j) => (
              <li key={j.id}>
                <span>{j.label}</span>
                <span className="muted">
                  {j.running ? "in corso" : st.nodes.some((n) => n.online) ? "in attesa" : "aspetta un server operazioni"}
                  {j.attempts > 0 ? ` · tentativi falliti: ${j.attempts}` : ""}
                  {!j.running ? waitText(j.retry_in) : ""}
                </span>
                {j.last_error && <span className="err">{j.last_error}</span>}
              </li>
            ))}
          </ul>
        )}
      </section>

      <section className="stor-card">
        <h2>Aggiungi un server operazioni</h2>
        <form
          className="stor-row"
          onSubmit={(e) => {
            e.preventDefault();
            void act(async () => {
              const r = (await call("/api/ops/nodes", "POST", { name: newName.trim() || "Operazioni" })) as { name: string; key: string };
              setShownKey({ name: r.name, key: r.key });
              setNewName("");
            });
          }}
        >
          <label>
            Nome
            <input value={newName} onChange={(e) => setNewName(e.target.value)} placeholder="PC di casa" maxLength={60} />
          </label>
          <button type="submit" className="primary">
            Crea chiave
          </button>
        </form>
      </section>
      {err && <p className="err">{err}</p>}
    </div>
  );
}

function OpsCard({ n, act, onKey }: { n: OpsNode; act: (f: () => Promise<unknown>) => Promise<void>; onKey: (key: string) => void }) {
  const [renaming, setRenaming] = useState(false);
  const [name, setName] = useState(n.name);
  const [confirm, setConfirm] = useState(false);
  const [auto, setAuto] = useState(n.parallel == null);
  const [parallel, setParallel] = useState(n.parallel ?? n.limit ?? 1);
  const maxParallel = Math.max(1, Math.min(8, n.slots || 8));
  return (
    <section className="stor-card">
      <div className="stor-head">
        {renaming ? (
          <form
            className="stor-rename"
            onSubmit={(e) => {
              e.preventDefault();
              const v = name.trim();
              if (!v) return;
              void act(() => call(`/api/ops/nodes/${n.id}`, "PATCH", { name: v })).then(() => setRenaming(false));
            }}
          >
            <input value={name} onChange={(e) => setName(e.target.value)} maxLength={40} autoFocus aria-label="Nome del server" />
            <button type="submit" className="small">
              Salva
            </button>
            <button
              type="button"
              className="ghost small"
              onClick={() => {
                setName(n.name);
                setRenaming(false);
              }}
            >
              Annulla
            </button>
          </form>
        ) : (
          <h2>
            <span className={`stor-dot${n.online ? " on" : ""}`} aria-hidden="true" />
            {n.name}
            <button type="button" className="ghost small" onClick={() => setRenaming(true)}>
              Rinomina
            </button>
          </h2>
        )}
        <span className="muted">{n.online ? `Online · v${n.version ?? "?"}` : "Offline"}</span>
      </div>
      <div className="stor-grid">
        <div>
          <span className="stor-k">
            Lavori in corso ({n.jobs.length} di {n.limit})
          </span>
          {n.jobs.length > 0 ? (
            n.jobs.map((j, i) => (
              <div key={i} className="ops-job">
                <span className="stor-v">{j.label}</span>
                <span className="stor-bar" aria-hidden="true">
                  <span style={{ width: `${j.pct}%` }} />
                </span>
                <span className="stor-sub">
                  {STAGES[j.stage] ?? j.stage} · {j.pct}%
                </span>
              </div>
            ))
          ) : (
            <span className="stor-v muted">{n.online ? "Libero" : "—"}</span>
          )}
        </div>
        <div>
          <span className="stor-k">Processore</span>
          <span className="stor-v">
            {n.threads ? `${n.threads} thread` : "—"} <span className="muted">· carico {n.load.toFixed(1)}</span>
          </span>
          <span className="stor-sub">{n.ffmpeg ?? (n.online ? "ffmpeg non trovato" : "")}</span>
        </div>
      </div>
      <form
        className="stor-row"
        onSubmit={(e) => {
          e.preventDefault();
          void act(() => call(`/api/ops/nodes/${n.id}`, "PATCH", auto ? { auto: true } : { parallel }));
        }}
      >
        <label className="stor-check">
          <input type="checkbox" checked={auto} onChange={(e) => setAuto(e.target.checked)} />
          Lavori contemporanei automatici{n.threads ? ` (${Math.max(1, Math.min(4, Math.floor(n.threads / 6)))} con ${n.threads} thread)` : ""}
        </label>
        {!auto && (
          <label className="stor-range">
            Lavori contemporanei: <strong>{parallel}</strong>
            <input type="range" min={1} max={maxParallel} value={Math.min(parallel, maxParallel)} onChange={(e) => setParallel(Number(e.target.value))} />
          </label>
        )}
        <button type="submit">Salva</button>
      </form>
      <div className="stor-row">
        {confirm ? (
          <>
            <span>Eliminare questo server operazioni? La sua chiave smette di funzionare.</span>
            <button type="button" className="danger" onClick={() => void act(() => call(`/api/ops/nodes/${n.id}`, "DELETE")).then(() => setConfirm(false))}>
              Sì
            </button>
            <button type="button" className="ghost" onClick={() => setConfirm(false)}>
              No
            </button>
          </>
        ) : (
          <>
            <button
              type="button"
              className="ghost"
              onClick={() =>
                void act(async () => {
                  const r = (await call(`/api/ops/nodes/${n.id}/key`, "POST")) as { key: string };
                  onKey(r.key);
                })
              }
            >
              Nuova chiave
            </button>
            <button type="button" className="danger" onClick={() => setConfirm(true)}>
              Elimina server
            </button>
          </>
        )}
      </div>
    </section>
  );
}
