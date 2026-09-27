import type { CustomPage } from "./custom-pages";
import type { CodenameModDetail, FunkinCatalog } from "./types";
import impostorLegacy from "./custom/impostor-legacy.json";
import mariosMadness from "./custom/marios-madness.json";
import pibbyApocalypse from "./custom/pibby-apocalypse.json";
import wiiFunkin from "./custom/wii-funkin.json";
import indieCross from "./custom/indie-cross.json";

const CATALOGS: Record<CustomPage, FunkinCatalog> = {
  "impostor-legacy": impostorLegacy as unknown as FunkinCatalog,
  "marios-madness": mariosMadness as unknown as FunkinCatalog,
  "pibby-apocalypse": pibbyApocalypse as unknown as FunkinCatalog,
  "wii-funkin": wiiFunkin as unknown as FunkinCatalog,
  "indie-cross": indieCross as unknown as FunkinCatalog,
};

export const withCustomCatalog = (detail: CodenameModDetail, page: CustomPage): CodenameModDetail => ({
  ...detail,
  mod: { ...detail.mod, catalog: CATALOGS[page] },
});
