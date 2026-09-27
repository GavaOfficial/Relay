import { Luckiest_Guy, M_PLUS_Rounded_1c, Special_Elite, VT323 } from "next/font/google";
import { apiGet } from "@/lib/api";
import { withCustomCatalog } from "@/lib/custom-catalogs";
import { customPageFor } from "@/lib/custom-pages";
import type { CodenameModDetail } from "@/lib/types";
import FunkinModView from "@/components/FunkinModView";
import MariosMadnessView from "@/components/custom/MariosMadnessView";
import PibbyView from "@/components/custom/PibbyView";
import WiiFunkinView from "@/components/custom/WiiFunkinView";

export const dynamic = "force-dynamic";

const mario = Luckiest_Guy({ weight: "400", subsets: ["latin"], variable: "--font-mario" });
const pibby = Special_Elite({ weight: "400", subsets: ["latin"], variable: "--font-pibby" });
const vcr = VT323({ weight: "400", subsets: ["latin"], variable: "--font-vcr" });
const wii = M_PLUS_Rounded_1c({ weight: ["500", "800"], subsets: ["latin"], variable: "--font-wii" });

export default async function PsychModPage({ params }: { params: Promise<{ mod: string }> }) {
  const { mod } = await params;
  const detail = await apiGet<CodenameModDetail>(`/api/psych/mods/${encodeURIComponent(mod)}`);
  const custom = customPageFor("psych", detail.mod.key);
  if (custom === "marios-madness") {
    return <MariosMadnessView initial={withCustomCatalog(detail, custom)} engine="psych" fontClass={mario.variable} />;
  }
  if (custom === "pibby-apocalypse") {
    return <PibbyView initial={withCustomCatalog(detail, custom)} engine="psych" fontClass={`${pibby.variable} ${vcr.variable}`} />;
  }
  if (custom === "wii-funkin") {
    return <WiiFunkinView initial={withCustomCatalog(detail, custom)} engine="psych" fontClass={wii.variable} />;
  }
  return <FunkinModView initial={detail} engine="psych" />;
}
