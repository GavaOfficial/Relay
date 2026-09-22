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
};

export type MatchDetail = MatchInfo & {
  connected: string[];

  videos?: string[];

  web_videos?: string[];

  vod_videos?: string[];

  processing?: Record<string, { stage: "queue" | "video" | "web"; pct: number }>;
};
