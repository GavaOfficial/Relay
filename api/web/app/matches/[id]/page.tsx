import { apiGet, currentIdentity } from "@/lib/api";
import type { MatchDetail } from "@/lib/types";
import MatchView from "@/components/MatchView";

export const dynamic = "force-dynamic";

export default async function MatchPage({ params }: { params: Promise<{ id: string }> }) {
  const { id } = await params;
  const [match, me] = await Promise.all([
    apiGet<MatchDetail>(`/api/matches/${encodeURIComponent(id)}`),
    currentIdentity(),
  ]);

  return <MatchView initial={match} canRename={me?.user === match.coordinator} nowMs={new Date().getTime()} />;
}
