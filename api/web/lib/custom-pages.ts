export const CUSTOM_PAGES = {
  "nmv:impostorlegacy": "impostor-legacy",
  "psych:mario": "marios-madness",
  "psych:pibby-apocalypse": "pibby-apocalypse",
  "psych:wii-funkin-vs-matt": "wii-funkin",
  "kade:indie-cross": "indie-cross",
} as const;

export type CustomPage = (typeof CUSTOM_PAGES)[keyof typeof CUSTOM_PAGES];

export const customPageFor = (engine: string, key: string): CustomPage | undefined =>
  (CUSTOM_PAGES as Record<string, CustomPage>)[`${engine}:${key}`];
