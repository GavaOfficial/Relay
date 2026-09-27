import type { CustomPage } from "./custom-pages";

export const customAsset = (page: CustomPage, name: string) => `/mods/${page}/${name.replace(/\.png$/i, ".webp")}`;
