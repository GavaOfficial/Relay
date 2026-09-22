import { notFound } from "next/navigation";
import ShareView from "@/components/ShareView";
import { apiGetShare } from "@/lib/api";
import type { MatchDetail } from "@/lib/types";

export const dynamic = "force-dynamic";

export default async function SharePage({ params }: { params: Promise<{ token: string }> }) {
  const { token } = await params;
  const match = await apiGetShare<MatchDetail>(`/api/share/${encodeURIComponent(token)}`);
  if (!match) notFound();

  return <ShareView initial={match} token={token} nowMs={new Date().getTime()} />;
}
