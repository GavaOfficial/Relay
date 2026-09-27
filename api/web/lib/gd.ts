export const GD_LOGO = "/img/geometry-dash.png";
export const GD_SECTIONS = ["Livelli principali", "Livelli online", "Platformer"];

const GDB = "https://gdbrowser.com";

export type GdLevel = {
  type?: number;
  difficulty?: number;
  average_difficulty?: number;
  demon?: boolean;
  demon_difficulty?: number;
  auto?: boolean;
  stars?: number;
  featured?: number;
  epic?: number;
  coins?: number;
  coins_verified?: boolean;
  length?: number;
  platformer?: boolean;
  attempts?: number;
  jumps?: number;
  best_percent?: number;
  best_time?: number;
};

export type GdAttempt = { percent?: number; completed?: boolean; coins?: number; attempt?: number; time_ms?: number };

export type GdProfile = {
  username?: string;
  account_id?: number;
  stars?: number;
  moons?: number;
  demons?: number;
  secret_coins?: number;
  user_coins?: number;
  diamonds?: number;
  jumps?: number;
  attempts?: number;
};

export const gdIcon = (name: string) => `${GDB}/assets/${name}.png`;
export const playerIcon = (username: string) => `${GDB}/icon/${encodeURIComponent(username)}`;
export const levelThumb = (id: string) => `https://levelthumbs.prevter.me/thumbnail/${encodeURIComponent(id)}/small`;

const DEMONS: Record<number, string> = { 3: "easy", 4: "medium", 5: "insane", 6: "extreme" };
const FACES = ["unrated", "easy", "normal", "hard", "harder", "insane"];
const RATING = ["", "epic", "legendary", "mythic"];

export function difficultyFace(l: GdLevel): { src: string; label: string } {
  let name: string;
  let label: string;
  if (l.demon) {
    const d = DEMONS[l.demon_difficulty ?? 0] ?? "hard";
    name = `demon-${d}`;
    label = `Demon ${d}`;
  } else if (l.auto) {
    name = "auto";
    label = "Auto";
  } else {
    const d = l.difficulty && l.difficulty > 0 ? l.difficulty : (l.average_difficulty ?? 0);
    name = FACES[Math.min(Math.max(d, 0), 5)] ?? "unrated";
    label = name === "unrated" ? "Senza difficoltà" : name[0].toUpperCase() + name.slice(1);
  }
  const rating = RATING[l.epic ?? 0] || ((l.featured ?? 0) > 0 ? "featured" : "");
  return { src: `${GDB}/assets/difficulties/${name}${rating ? `-${rating}` : ""}.png`, label };
}

export function formatTime(ms?: number): string {
  const t = Math.max(0, Math.round(ms ?? 0));
  const m = Math.floor(t / 60000);
  const s = Math.floor(t / 1000) % 60;
  const cs = Math.floor((t % 1000) / 10);
  return `${m}:${String(s).padStart(2, "0")}.${String(cs).padStart(2, "0")}`;
}

export function resultText(a: GdAttempt | undefined, platformer: boolean): string {
  if (!a) return "—";
  if (a.completed) return platformer ? formatTime(a.time_ms) : "Completato";
  return `${a.percent ?? 0}%`;
}

export const formatNumber = (n?: number) => (n ?? 0).toLocaleString("it-IT");
