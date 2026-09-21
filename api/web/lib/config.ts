import "server-only";

const strip = (s: string) => s.replace(/\/+$/, "");

export const GAVAAUTH_URL = process.env.GAVAAUTH_URL ? strip(process.env.GAVAAUTH_URL) : "";
export const GAVAAUTH_CLIENT_ID = process.env.GAVAAUTH_CLIENT_ID || "relay";

export const SITE_URL = strip(process.env.SITE_URL || "http://localhost:3000");
export const REDIRECT_URI = `${SITE_URL}/auth/callback`;
export const DEV_LOGIN = process.env.DEV_LOGIN === "1";
export const PKCE_COOKIE = "relay_pkce";
export const SECURE_COOKIES = process.env.NODE_ENV === "production";
