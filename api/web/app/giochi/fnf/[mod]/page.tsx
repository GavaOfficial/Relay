import { apiGet } from "@/lib/api";
import type { CodenameModDetail } from "@/lib/types";
import FnfModView from "@/components/FnfModView";

export const dynamic = "force-dynamic";

export default async function FnfModPage({ params }: { params: Promise<{ mod: string }> }) {
  const { mod } = await params;
  const detail = await apiGet<CodenameModDetail>(`/api/codename/mods/${encodeURIComponent(mod)}`);
  return <FnfModView initial={detail} />;
}
