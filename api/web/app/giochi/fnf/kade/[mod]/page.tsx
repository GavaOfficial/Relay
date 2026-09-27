import { Oswald } from "next/font/google";
import { apiGet } from "@/lib/api";
import { withCustomCatalog } from "@/lib/custom-catalogs";
import { customPageFor } from "@/lib/custom-pages";
import type { CodenameModDetail } from "@/lib/types";
import FunkinModView from "@/components/FunkinModView";
import IndieCrossView from "@/components/custom/IndieCrossView";

export const dynamic = "force-dynamic";

const bronx = Oswald({ weight: ["500"], subsets: ["latin"], variable: "--font-ic" });

export default async function KadeModPage({ params }: { params: Promise<{ mod: string }> }) {
  const { mod } = await params;
  const detail = await apiGet<CodenameModDetail>(`/api/kade/mods/${encodeURIComponent(mod)}`);
  if (customPageFor("kade", detail.mod.key) === "indie-cross") {
    return <IndieCrossView initial={withCustomCatalog(detail, "indie-cross")} engine="kade" fontClass={bronx.variable} />;
  }
  return <FunkinModView initial={detail} engine="kade" />;
}
