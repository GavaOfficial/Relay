import Link from "next/link";
import { apiGetOr } from "@/lib/api";
import { FNF_LOGO } from "@/lib/codename";
import { GD_LOGO } from "@/lib/gd";
import type { CodenameModSummary } from "@/lib/types";

export const dynamic = "force-dynamic";

export default async function GamesPage() {
  const list = (engine: string) => apiGetOr<CodenameModSummary[]>(`/api/${engine}/mods`, []);
  const [codename, funkin, psych, nmv, kade, gd] = await Promise.all(["codename", "funkin", "psych", "nmv", "kade", "gd"].map(list));
  const mods = [...funkin, ...codename, ...psych, ...nmv, ...kade].filter((m) => m.songs > 0);
  const records = mods.reduce((n, m) => n + m.songs, 0);
  const gdRecords = gd.reduce((n, m) => n + m.songs, 0);

  return (
    <>
      <div className="pagehead">
        <h1>Giochi</h1>
        <p className="muted">Le integrazioni di Relay: registrano da sole i momenti migliori mentre giochi.</p>
      </div>

      <ul className="gametiles">
        <li>
          <Link href="/giochi/fnf" className="gametile">
            {/* eslint-disable-next-line @next/next/no-img-element */}
            <img src={FNF_LOGO} alt="" className="gametile-logo" />
            <div className="gametile-body">
              <span className="gametile-kicker">Rhythm game</span>
              <strong>Friday Night Funkin&apos;</strong>
              <span className="muted">
                {mods.length ? `${mods.length} mod · ${records} record` : "Nessun record ancora"}
              </span>
            </div>
            <svg className="gametile-go" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <path d="m9 18 6-6-6-6" />
            </svg>
          </Link>
        </li>
        <li>
          <Link href="/giochi/gd" className="gametile">
            {/* eslint-disable-next-line @next/next/no-img-element */}
            <img src={GD_LOGO} alt="" className="gametile-logo" />
            <div className="gametile-body">
              <span className="gametile-kicker">Platform</span>
              <strong>Geometry Dash</strong>
              <span className="muted">{gdRecords ? `${gdRecords} record` : "Nessun record ancora"}</span>
            </div>
            <svg className="gametile-go" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <path d="m9 18 6-6-6-6" />
            </svg>
          </Link>
        </li>
        <li>
          <div className="gametile soon" aria-disabled="true">
            <span className="gametile-logo soon-mark" aria-hidden="true">+</span>
            <div className="gametile-body">
              <span className="gametile-kicker">In arrivo</span>
              <strong>Altri giochi</strong>
              <span className="muted">Nuove integrazioni arriveranno qui.</span>
            </div>
          </div>
        </li>
      </ul>
    </>
  );
}
