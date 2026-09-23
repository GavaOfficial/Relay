import Link from "next/link";
import { AvatarStack } from "@/components/Avatar";
import When from "@/components/When";
import { apiGet } from "@/lib/api";
import { isLive as liveNow } from "@/lib/live";
import { formatTime } from "@/lib/sync";
import type { MatchListItem } from "@/lib/types";

export const dynamic = "force-dynamic";

export default async function Home() {
  const all = await apiGet<MatchListItem[]>("/api/matches");
  const now = new Date().getTime();
  const isLive = (m: MatchListItem) => liveNow(m, now);

  const matches = all
    .filter((m) => isLive(m) || m.has_recording)
    .sort((a, b) => Number(isLive(b)) - Number(isLive(a)) || b.created_at - a.created_at);

  return (
    <>
      <div className="pagehead">
        <h1>Partite</h1>
      </div>

      <div className="homelist">
      {matches.length === 0 ? (
        <div className="emptystate">
          <span className="bigdot" aria-hidden="true" />
          <p>Nessuna registrazione, per ora.</p>
          <p className="muted">Le partite si creano dall&apos;app Relay: quando una è in diretta o finita, compare qui.</p>
        </div>
      ) : (
        <ul className="grid">
          {matches.map((m) => {
            const names = m.players.map((id) => m.names[id] ?? id);
            const live = isLive(m);
            return (
              <li key={m.id}>
                <Link href={`/matches/${m.id}`} className="mcard">
                  <div className="thumb">
                    {m.has_thumb ? (
                      // eslint-disable-next-line @next/next/no-img-element
                      <img src={`/api/matches/${m.id}/thumb.jpg`} alt="" loading="lazy" />
                    ) : (
                      <svg className="ph" width="34" height="34" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
                        <path d="M8 5.5v13a1 1 0 0 0 1.5.86l10.5-6.5a1 1 0 0 0 0-1.72L9.5 4.64A1 1 0 0 0 8 5.5z" />
                      </svg>
                    )}
                    {m.game_name && (
                      <div className="thumb-scrim">
                        <span>{m.game_name}</span>
                      </div>
                    )}
                    {live ? (
                      <span className="badge live">Diretta</span>
                    ) : (
                      m.duration_secs != null && <span className="badge">{formatTime(m.duration_secs)}</span>
                    )}
                  </div>
                  <div className="mcard-body">
                    <span className="mcard-title">{m.name ?? (names.length ? names.join(", ") : "Partita")}</span>
                    <span className="mcard-meta">
                      <AvatarStack names={names} max={5} />
                      <When seconds={m.created_at} />
                    </span>
                  </div>
                </Link>
              </li>
            );
          })}
        </ul>
      )}
      </div>
    </>
  );
}
