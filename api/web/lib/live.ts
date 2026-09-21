export const LIVE_WINDOW_MS = 6 * 60 * 60 * 1000;

export function isLive(
  m: { started_at_ms: number | null; stopped_at_ms: number | null; status: string },
  nowMs: number,
): boolean {
  return m.started_at_ms !== null && m.stopped_at_ms === null && m.status !== "ended" && nowMs - m.started_at_ms < LIVE_WINDOW_MS;
}
