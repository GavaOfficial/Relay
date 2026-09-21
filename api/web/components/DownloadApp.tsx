import { latestApp } from "@/lib/api";

const mb = (bytes: number) => `${Math.max(1, Math.round(bytes / 1e6))} MB`;

function DownloadIcon() {
  return (
    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M12 4v11m0 0-4-4m4 4 4-4M5 20h14" />
    </svg>
  );
}

export default async function DownloadApp({ variant = "compact" }: { variant?: "compact" | "full" }) {
  const rel = await latestApp();
  if (!rel) return null;

  if (variant === "compact") {
    return (
      <a href="/api/app/download" className="dlapp" download title={`Relay per Windows · versione ${rel.version} (${mb(rel.size)})`}>
        <DownloadIcon />
        Scarica l&apos;app
      </a>
    );
  }
  return (
    <a href="/api/app/download" className="dlcard" download>
      <span className="dlcard-ic">
        <DownloadIcon />
      </span>
      <span className="dlcard-txt">
        <strong>Scarica Relay per Windows</strong>
        <span className="muted">
          Versione {rel.version} · {mb(rel.size)} · si aggiorna da sola
        </span>
      </span>
    </a>
  );
}
