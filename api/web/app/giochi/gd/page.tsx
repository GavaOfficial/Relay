import { apiGetOr } from "@/lib/api";
import { GD_SECTIONS } from "@/lib/gd";
import type { GdProfile } from "@/lib/gd";
import type { CodenameModDetail, CodenameModSummary } from "@/lib/types";
import GdView from "@/components/GdView";

export const dynamic = "force-dynamic";

export default async function GeometryDashPage() {
  const [profile, mods] = await Promise.all([
    apiGetOr<GdProfile | null>("/api/gd/profile", null),
    apiGetOr<CodenameModSummary[]>("/api/gd/mods", []),
  ]);
  const details = await Promise.all(
    mods.map((m) => apiGetOr<CodenameModDetail | null>(`/api/gd/mods/${encodeURIComponent(m.key)}`, null)),
  );
  const sections = details
    .filter((d): d is CodenameModDetail => d !== null)
    .sort((a, b) => {
      const ia = GD_SECTIONS.indexOf(a.mod.name);
      const ib = GD_SECTIONS.indexOf(b.mod.name);
      return (ia < 0 ? 99 : ia) - (ib < 0 ? 99 : ib);
    });
  return <GdView profile={profile} sections={sections} />;
}
