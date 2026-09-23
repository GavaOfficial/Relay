import { isLive } from "@/lib/live";
import type { MatchDetail } from "@/lib/types";

export function statusOf(m: MatchDetail, nowMs: number): { label: string; cls: string } {
  if (m.status === "ended") return { label: "Terminata", cls: "" };
  if (m.stopped_at_ms) return { label: "Caricamento", cls: "" };
  if (m.started_at_ms) return { label: isLive(m, nowMs) ? "In diretta" : "Registrata", cls: isLive(m, nowMs) ? "live" : "" };
  return { label: "In attesa", cls: "idle" };
}

export function stageLabel(stage: "queue" | "video" | "web", pct: number): string {
  if (stage === "queue") return "In coda";
  if (stage === "web") return `Versione leggera ${pct}%`;
  return `Preparo il video ${pct}%`;
}

export function steamLibraryCover(appId?: number | null): string | undefined {
  return appId ? `https://cdn.akamai.steamstatic.com/steam/apps/${appId}/library_600x900.jpg` : undefined;
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(v >= 10 ? 0 : 1)} ${units[i]}`;
}
