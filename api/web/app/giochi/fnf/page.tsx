import Link from "next/link";
import { apiGetOr } from "@/lib/api";
import { BASE_GAME_COVER, BASE_GAME_KEY, FNF_LOGO, assetUrl, formatDay, modCover, modHref, modTitle, type Engine } from "@/lib/codename";
import type { CodenameModSummary } from "@/lib/types";

export const dynamic = "force-dynamic";

type Card = CodenameModSummary & { engine: Engine };

function ModCard({ m }: { m: Card }) {
  const isBase = m.engine === "funkin" && m.key === BASE_GAME_KEY;
  const icon = m.engine !== "codename" && m.has_icon ? assetUrl(m.engine, m.key, "icon.png") : undefined;
  const cover = isBase ? BASE_GAME_COVER : (m.gb_cover_url ?? (icon ? undefined : modCover(m, m.preview_clip, m.engine)));
  const count = m.tracks
    ? `${m.songs}/${m.tracks} canzoni`
    : m.songs === 1
      ? "1 canzone"
      : `${m.songs} canzoni`;
  return (
    <li>
      <Link href={modHref(m.engine, m.key)} className="mcard">
        <div className={"thumb" + (m.songs === 0 && !isBase ? " fk-unplayed" : "")}>
          {cover ? (
            // eslint-disable-next-line @next/next/no-img-element
            <img src={cover} alt="" loading="lazy" />
          ) : icon ? (
            <>
              {/* eslint-disable-next-line @next/next/no-img-element */}
              <img src={icon} alt="" aria-hidden="true" className="fk-cover-blur" />
              {/* eslint-disable-next-line @next/next/no-img-element */}
              <img src={icon} alt="" loading="lazy" className="fk-thumb-icon" />
            </>
          ) : (
            // eslint-disable-next-line @next/next/no-img-element
            <img src={FNF_LOGO} alt="" className="thumb-logo" />
          )}
          <span className="badge">{count}</span>
        </div>
        <div className="mcard-body">
          <div className="mcard-title">{isBase ? "Gioco base" : modTitle(m)}</div>
          <div className="mcard-meta">
            <span>{isBase ? "Friday Night Funkin'" : m.gb_author ? `di ${m.gb_author}` : "Mod"}</span>
            <span>{m.last_at ? formatDay(m.last_at) : "Da giocare"}</span>
          </div>
        </div>
      </Link>
    </li>
  );
}

async function mods(engine: Engine): Promise<Card[]> {
  const list = await apiGetOr<CodenameModSummary[]>(`/api/${engine}/mods`, []);
  return list.map((m) => ({ ...m, engine }));
}

export default async function FnfPage() {
  const [funkin, codename, allPsych, allNmv, allKade] = await Promise.all([mods("funkin"), mods("codename"), mods("psych"), mods("nmv"), mods("kade")]);
  const base = funkin.filter((m) => m.key === BASE_GAME_KEY);
  const others = [...funkin.filter((m) => m.key !== BASE_GAME_KEY), ...codename, ...allPsych, ...allNmv, ...allKade]
    .filter((m) => m.songs > 0)
    .sort((a, b) => b.last_at - a.last_at || modTitle(a).localeCompare(modTitle(b)));
  const all = [...base, ...others];

  return (
    <>
      <Link href="/giochi" className="back">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
          <path d="m15 18-6-6 6-6" />
        </svg>
        Giochi
      </Link>

      <div className="pagehead gamehead">
        {/* eslint-disable-next-line @next/next/no-img-element */}
        <img src={FNF_LOGO} alt="" />
        <div>
          <h1>Friday Night Funkin&apos;</h1>
          <p className="muted">Il gioco, le tue mod e i tuoi record, canzone per canzone.</p>
        </div>
      </div>

      {all.length === 0 ? (
        <div className="emptystate">
          {/* eslint-disable-next-line @next/next/no-img-element */}
          <img src={FNF_LOGO} alt="" className="empty-logo" />
          <p>Nessun record, per ora.</p>
          <p className="muted">
            Nell&apos;app Relay vai in Impostazioni → Connettori, collega Friday Night Funkin&apos; e gioca
            una canzone: quando fai un record la clip compare qui.
          </p>
        </div>
      ) : (
        <ul className="grid">
          {all.map((m) => (
            <ModCard key={`${m.engine}-${m.key}`} m={m} />
          ))}
        </ul>
      )}
    </>
  );
}
