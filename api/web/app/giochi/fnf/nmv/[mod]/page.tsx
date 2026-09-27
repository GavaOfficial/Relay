import { Amatic_SC, VT323 } from "next/font/google";
import { apiGet } from "@/lib/api";
import { withCustomCatalog } from "@/lib/custom-catalogs";
import { customPageFor } from "@/lib/custom-pages";
import type { CodenameModDetail } from "@/lib/types";
import FunkinModView from "@/components/FunkinModView";
import ImpostorLegacyView from "@/components/custom/ImpostorLegacyView";

export const dynamic = "force-dynamic";

const amatic = Amatic_SC({ weight: "700", subsets: ["latin"], variable: "--font-amatic" });
const vcr = VT323({ weight: "400", subsets: ["latin"], variable: "--font-vcr" });

export default async function NmvModPage({ params }: { params: Promise<{ mod: string }> }) {
  const { mod } = await params;
  const detail = await apiGet<CodenameModDetail>(`/api/nmv/mods/${encodeURIComponent(mod)}`);
  if (customPageFor("nmv", detail.mod.key) === "impostor-legacy") {
    return <ImpostorLegacyView initial={withCustomCatalog(detail, "impostor-legacy")} engine="nmv" fontClass={`${amatic.variable} ${vcr.variable}`} />;
  }
  return <FunkinModView initial={detail} engine="nmv" />;
}
