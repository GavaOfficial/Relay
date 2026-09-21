export const DRIFT_SEEK = 0.3;

export const DRIFT_NUDGE = 0.08;

const NUDGE_RATE = 0.05;

export const LIVE_MARGIN = 10;

export type DriftAction = { seekTo: number | null; rate: number };

export function driftAction(masterTime: number, slaveTime: number): DriftAction {
  const d = slaveTime - masterTime;
  const abs = Math.abs(d);
  if (abs > DRIFT_SEEK) return { seekTo: masterTime, rate: 1 };
  if (abs > DRIFT_NUDGE) return { seekTo: null, rate: d > 0 ? 1 - NUDGE_RATE : 1 + NUDGE_RATE };
  return { seekTo: null, rate: 1 };
}

export function commonEnd(ends: (number | null | undefined)[]): number | null {
  const finite = ends.filter((e): e is number => typeof e === "number" && Number.isFinite(e));
  return finite.length ? Math.min(...finite) : null;
}

export function longestEnd(ends: (number | null | undefined)[]): number | null {
  const finite = ends.filter((e): e is number => typeof e === "number" && Number.isFinite(e));
  return finite.length ? Math.max(...finite) : null;
}

export function livePosition(end: number, margin = LIVE_MARGIN): number {
  return Math.max(0, end - margin);
}

export function clamp(t: number, min: number, max: number): number {
  return Math.min(Math.max(t, min), max);
}

export function formatTime(seconds: number): string {
  const s = Math.max(0, Math.floor(Number.isFinite(seconds) ? seconds : 0));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  const mm = h > 0 ? String(m).padStart(2, "0") : String(m);
  return `${h > 0 ? h + ":" : ""}${mm}:${String(sec).padStart(2, "0")}`;
}
