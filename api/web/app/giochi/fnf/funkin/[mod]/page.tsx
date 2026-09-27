import { apiGet } from "@/lib/api";
import type { CodenameModDetail } from "@/lib/types";
import FunkinModView from "@/components/FunkinModView";

export const dynamic = "force-dynamic";

export default async function FunkinModPage({ params }: { params: Promise<{ mod: string }> }) {
  const { mod } = await params;
  const detail = await apiGet<CodenameModDetail>(`/api/funkin/mods/${encodeURIComponent(mod)}`);
  return <FunkinModView initial={detail} />;
}
