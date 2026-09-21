import { NextResponse, type NextRequest } from "next/server";
import { API_ORIGIN, SESSION_COOKIE } from "@/lib/api";
import {
  GAVAAUTH_CLIENT_ID,
  GAVAAUTH_URL,
  PKCE_COOKIE,
  REDIRECT_URI,
  SECURE_COOKIES,
  SITE_URL,
} from "@/lib/config";
import { safeEqual } from "@/lib/pkce";
import { safeNext } from "@/lib/redirect";

export const dynamic = "force-dynamic";

function fail(code: string) {
  const res = NextResponse.redirect(`${SITE_URL}/login?error=${code}`);
  res.cookies.delete({ name: PKCE_COOKIE, path: "/auth" });
  return res;
}

type Pkce = { verifier: string; state: string; next: string };

function readPkce(raw: string | undefined): Pkce | null {
  if (!raw) return null;
  try {
    const p = JSON.parse(raw) as Partial<Pkce>;
    if (typeof p.verifier === "string" && typeof p.state === "string") {
      return { verifier: p.verifier, state: p.state, next: safeNext(p.next) };
    }
  } catch {
  }
  return null;
}

export async function GET(req: NextRequest) {
  if (!GAVAAUTH_URL) return fail("config");

  const q = req.nextUrl.searchParams;
  if (q.get("error")) return fail("denied");

  const pkce = readPkce(req.cookies.get(PKCE_COOKIE)?.value);
  const state = q.get("state");
  const code = q.get("code");
  if (!pkce || !state || !safeEqual(state, pkce.state)) return fail("state");
  if (!code) return fail("exchange");

  let accessToken: string;
  try {
    const r = await fetch(`${GAVAAUTH_URL}/token`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        grant_type: "authorization_code",
        code,
        client_id: GAVAAUTH_CLIENT_ID,
        redirect_uri: REDIRECT_URI,
        code_verifier: pkce.verifier,
      }),
      cache: "no-store",
    });
    if (!r.ok) return fail("exchange");
    const j = (await r.json()) as { access_token?: string };
    if (!j.access_token) return fail("exchange");
    accessToken = j.access_token;
  } catch {
    return fail("exchange");
  }

  let session: { session: string; expires_in: number };
  try {
    const r = await fetch(`${API_ORIGIN}/api/session`, {
      method: "POST",
      headers: { authorization: `Bearer ${accessToken}` },
      cache: "no-store",
    });
    if (r.status === 401) return fail("session");
    if (!r.ok) return fail("server");
    session = (await r.json()) as { session: string; expires_in: number };
  } catch {
    return fail("server");
  }

  const res = NextResponse.redirect(`${SITE_URL}${pkce.next}`);
  res.cookies.set(SESSION_COOKIE, session.session, {
    httpOnly: true,
    sameSite: "lax",
    secure: SECURE_COOKIES,
    path: "/",
    maxAge: session.expires_in,
  });
  res.cookies.delete({ name: PKCE_COOKIE, path: "/auth" });
  return res;
}
