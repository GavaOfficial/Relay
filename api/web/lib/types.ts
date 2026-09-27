export type Stage = "queue" | "download" | "video" | "web" | "upload";

export type MatchStatus = "open" | "ended";

export type MatchInfo = {
  id: string;

  name?: string;
  coordinator: string;
  players: string[];

  names: Record<string, string>;

  invite_code?: string;
  share_token?: string;
  game_app_id?: number;
  game_name?: string;
  game_cover_url?: string;
  fnf_song_name?: string;
  fnf_difficulty?: string;
  fnf_score?: number;
  fnf_accuracy?: number;
  fnf_misses?: { at_ms: number }[];
  fnf_timeline?: { at_ms: number; song_name?: string; difficulty?: string; score?: number; accuracy?: number }[];
  finished: string[];
  status: MatchStatus;
  created_at: number;

  started_at_ms: number | null;
  stopped_at_ms: number | null;
};

export type MatchListItem = MatchInfo & {
  has_recording: boolean;

  duration_secs: number | null;
  has_thumb: boolean;
  processing?: boolean;
};

export type MatchDetail = MatchInfo & {
  connected: string[];

  videos?: string[];

  web_videos?: string[];

  vod_videos?: string[];

  processing?: Record<string, { stage: Stage; pct: number }>;
};

export type CodenameMod = {
  key: string;
  name: string;
  gamebanana_url?: string;
  gb_name?: string;
  gb_author?: string;
  gb_cover_url?: string;
  catalog?: FunkinCatalog;
};

export type FunkinCatalog = {
  title?: string;
  description?: string;
  version?: string;
  contributors: { name: string; role?: string; url?: string }[];
  has_icon: boolean;
  albums: { id: string; name: string; artists: string[]; art?: string }[];
  tracks: FunkinTrack[];
  extra?: Record<string, unknown>;
};

export type FunkinTrack = {
  id: string;
  variation?: string;
  name: string;
  artist?: string;
  album?: string;
  bpm?: number;
  difficulties: string[];
  ratings: Record<string, number>;
  icon?: string;
  color?: string;
  extra?: Record<string, unknown>;
};

export type CodenameModSummary = CodenameMod & {
  title?: string;
  has_icon?: boolean;
  tracks?: number;
  songs: number;
  clips: number;
  last_at: number;
  preview_clip: string | null;
};

export type CodenameClip = {
  id: string;
  mod_key: string;
  song_id?: string;
  song: string;
  difficulty: string;
  variation?: string;
  score: number;
  accuracy?: number | null;
  misses: number;
  duration_ms: number;
  recorded_at: number;
  archived: boolean;
  has_thumb: boolean;
  processing?: boolean;
  extra?: Record<string, unknown>;
};

export type CodenameSong = {
  song: string;
  song_id?: string;
  difficulty: string;
  variation?: string;
  best: CodenameClip;
  archive: CodenameClip[];
};

export type CodenameModDetail = {
  mod: CodenameMod;
  songs: CodenameSong[];
};
