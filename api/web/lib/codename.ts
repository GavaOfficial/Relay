import type { CodenameMod } from "./types";

export type Engine = "funkin" | "codename" | "psych" | "nmv" | "kade" | "gd";

export const ENGINE_LABEL: Record<Engine, string> = {
  funkin: "Friday Night Funkin'",
  codename: "Codename Engine",
  psych: "Psych Engine",
  nmv: "Nightmare Vision",
  kade: "Kade Engine",
  gd: "Geometry Dash",
};

export const FNF_LOGO = "/img/fnf.png";
export const BASE_GAME_KEY = "gioco-base";
export const BASE_GAME_COVER = "/img/fnf-base-cover.jpg";

export const modTitle = (m: CodenameMod & { title?: string }) => m.gb_name ?? m.title ?? m.catalog?.title ?? m.name;

export const modHref = (engine: Engine, key: string) =>
  engine === "codename" ? `/giochi/fnf/${encodeURIComponent(key)}` : `/giochi/fnf/${engine}/${encodeURIComponent(key)}`;

export const assetUrl = (engine: Engine, key: string, name: string) =>
  `/api/${engine}/mods/${encodeURIComponent(key)}/assets/${encodeURIComponent(name)}`;

export function modCover(m: CodenameMod, previewClip?: string | null, engine: Engine = "codename"): string | undefined {
  if (m.gb_cover_url) return m.gb_cover_url;
  return previewClip ? clipThumb(previewClip, engine) : undefined;
}

export const clipVideo = (id: string, engine: Engine = "codename") => `/api/${engine}/clips/${id}/video.mp4`;
export const clipThumb = (id: string, engine: Engine = "codename") => `/api/${engine}/clips/${id}/thumb.jpg`;

export const formatScore = (n: number) => n.toLocaleString("it-IT");

export const formatAccuracy = (a?: number | null) => (a == null ? "—" : `${(a * 100).toFixed(2)}%`);

export function formatClipLength(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

export const formatDay = (secs: number) =>
  new Date(secs * 1000).toLocaleDateString("it-IT", { day: "numeric", month: "short", year: "numeric" });

export const plainVariation = (v?: string | null) => (!v || v.toLowerCase() === "default" ? "" : v.toLowerCase());
