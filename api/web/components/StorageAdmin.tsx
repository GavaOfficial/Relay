"use client";

import { useCallback, useEffect, useState } from "react";

export type StorageNode = {
  id: string;
  name: string;
  limit: number | null;
  created_at: number;
  online: boolean;
  connections: number;
  version: string | null;
  since: number | null;
  disk_total: number;
  disk_free: number;
  used: number;
  files: number;
  up_rate: number;
  down_rate: number;
};

export type StorageStatus = {
  nodes: StorageNode[];
  cache_limit: number;
  activity: {
    queued: number;
    queued_bytes: number;
    current: string | null;
    last_error: string | null;
    cache_bytes: number;
    local_only_bytes: number;
  };
};

const GB = 1_000_000_000;

function size(bytes: number) {
  if (bytes >= 1e12) return `${(bytes / 1e12).toFixed(2)} TB`;
  if (bytes >= GB) return `${(bytes / GB).toFixed(1)} GB`;
  if (bytes >= 1e6) return `${(bytes / 1e6).toFixed(0)} MB`;
  return `${Math.round(bytes / 1e3)} KB`;
}

function speed(bps: number) {
  if (!bps) return "—";
  const mbit = (bps * 8) / 1e6;
  return `${mbit >= 10 ? mbit.toFixed(0) : mbit.toFixed(1)} Mb/s`;
}

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
  return `curl -fsSL ${origin}/api/storage/install.sh | sudo bash -s -- --key ${key} --server ${origin}`;
}

export default function StorageAdmin({ initial }: { initial: StorageStatus }) {
  const [st, setSt] = useState(initial);
  const [err, setErr] = useState<string | null>(null);
  const [newName, setNewName] = useState("");
  const [shownKey, setShownKey] = useState<{ name: string; key: string } | null>(null);
  const [cacheGb, setCacheGb] = useState(String(Math.round(initial.cache_limit / GB)));

  const refresh = useCallback(async () => {
    try {
      setSt((await call("/api/storage/status", "GET")) as StorageStatus);
    } catch {
      /* resta l'ultimo stato */
    }
  }, []);
  useEffect(() => {
    const t = window.setInterval(refresh, 5000);
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

  const addNode = () =>
    act(async () => {
      const r = (await call("/api/storage/nodes", "POST", { name: newName.trim() || "Server" })) as { name: string; key: string };
      setShownKey({ name: r.name, key: r.key });
      setNewName("");
    });

  const a = st.activity;
  return (
    <div className="stor">
      <section className="stor-card">
        <div className="stor-head">
          <h2>Server centrale</h2>
          <button type="button" className="primary small" onClick={() => act(() => call("/api/storage/migrate", "POST"))} disabled={!st.nodes.some((n) => n.online)}>
            Sposta tutto adesso
          </button>
        </div>
        <div className="stor-grid">
          <div>
            <span className="stor-k">Cache dei video</span>
            <span className="stor-v">
              {size(a.cache_bytes)} <span className="muted">di {size(st.cache_limit)}</span>
            </span>
            <Bar value={a.cache_bytes} max={st.cache_limit} />
          </div>
          <div>
            <span className="stor-k">Da spostare</span>
            <span className="stor-v">
              {a.queued} video <span className="muted">· {size(a.queued_bytes)}</span>
            </span>
            {a.current && <span className="stor-sub">In corso: {a.current.split("/").pop()}</span>}
          </div>
        </div>
        <form
          className="stor-row"
          onSubmit={(e) => {
            e.preventDefault();
            const gb = Number(cacheGb);
            if (Number.isFinite(gb) && gb >= 0) void act(() => call("/api/storage/cache", "PUT", { limit: Math.round(gb * GB) }));
          }}
        >
          <label>
            Spazio massimo per la cache sul centrale (GB)
            <input type="number" min={0} step={1} value={cacheGb} onChange={(e) => setCacheGb(e.target.value)} />
          </label>
          <button type="submit">Salva</button>
        </form>
        {a.last_error && <p className="err">Ultimo errore: {a.last_error}</p>}
      </section>

      {st.nodes.map((n) => (
        <NodeCard key={n.id} n={n} onChanged={refresh} act={act} onKey={(key) => setShownKey({ name: n.name, key })} />
      ))}

      {shownKey && (
        <section className="stor-card stor-key" role="alert">
          <h2>Chiave di «{shownKey.name}»</h2>
          <p className="muted">
            Si vede solo adesso: copiala. Sul server Ubuntu esegui questo comando (installa relay-storage come servizio e lo collega):
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
        <h2>Aggiungi un server</h2>
        <form
          className="stor-row"
          onSubmit={(e) => {
            e.preventDefault();
            void addNode();
          }}
        >
          <label>
            Nome
            <input value={newName} onChange={(e) => setNewName(e.target.value)} placeholder="Casa" maxLength={60} />
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

function Bar({ value, max, warn }: { value: number; max: number; warn?: boolean }) {
  const pct = max > 0 ? Math.min(100, (value / max) * 100) : 0;
  return (
    <span className={`stor-bar${warn || pct > 90 ? " warn" : ""}`} aria-hidden="true">
      <span style={{ width: `${pct}%` }} />
    </span>
  );
}

function NodeCard({
  n,
  act,
  onKey,
}: {
  n: StorageNode;
  onChanged: () => void;
  act: (f: () => Promise<unknown>) => Promise<void>;
  onKey: (key: string) => void;
}) {
  const maxGb = Math.max(1, Math.floor(n.disk_total / GB));
  const [limitGb, setLimitGb] = useState(n.limit != null ? Math.round(n.limit / GB) : maxGb);
  const [auto, setAuto] = useState(n.limit == null);
  const cap = n.limit ?? n.used + Math.max(0, n.disk_free - 5 * GB);
  return (
    <section className="stor-card">
      <div className="stor-head">
        <h2>
          <span className={`stor-dot${n.online ? " on" : ""}`} aria-hidden="true" />
          {n.name}
        </h2>
        <span className="muted">
          {n.online ? `Online · ${n.connections} connessioni · v${n.version}` : "Offline"}
        </span>
      </div>
      <div className="stor-grid">
        <div>
          <span className="stor-k">Video di Relay</span>
          <span className="stor-v">
            {size(n.used)} <span className="muted">· {n.files} file</span>
          </span>
          <Bar value={n.used} max={cap} />
          <span className="stor-sub">Limite: {n.limit != null ? size(n.limit) : `tutto lo spazio libero (${size(cap)})`}</span>
        </div>
        <div>
          <span className="stor-k">Disco</span>
          <span className="stor-v">
            {n.disk_total ? `${size(n.disk_free)} liberi` : "—"} <span className="muted">di {n.disk_total ? size(n.disk_total) : "—"}</span>
          </span>
          <Bar value={n.disk_total - n.disk_free} max={n.disk_total} />
        </div>
        <div>
          <span className="stor-k">Velocità</span>
          <span className="stor-v">
            ↑ {speed(n.up_rate)} <span className="muted">· ↓ {speed(n.down_rate)}</span>
          </span>
          <span className="stor-sub">↑ verso il server · ↓ video letti</span>
        </div>
      </div>
      <form
        className="stor-row"
        onSubmit={(e) => {
          e.preventDefault();
          void act(() => call(`/api/storage/nodes/${n.id}`, "PATCH", { limit: auto ? null : limitGb * GB }));
        }}
      >
        <label className="stor-check">
          <input type="checkbox" checked={auto} onChange={(e) => setAuto(e.target.checked)} />
          Usa tutto lo spazio libero
        </label>
        {!auto && (
          <label className="stor-range">
            Spazio per Relay: <strong>{limitGb} GB</strong>
            <input type="range" min={1} max={maxGb} value={Math.min(limitGb, maxGb)} onChange={(e) => setLimitGb(Number(e.target.value))} />
          </label>
        )}
        <button type="submit">Salva</button>
        <button
          type="button"
          className="ghost"
          onClick={() =>
            void act(async () => {
              const r = (await call(`/api/storage/nodes/${n.id}/key`, "POST")) as { key: string };
              onKey(r.key);
            })
          }
        >
          Nuova chiave
        </button>
      </form>
    </section>
  );
}
