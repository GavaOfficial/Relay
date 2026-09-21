import { NextResponse, type NextRequest } from "next/server";
import { GAVAAUTH_CLIENT_ID, GAVAAUTH_URL, PKCE_COOKIE, REDIRECT_URI, SECURE_COOKIES, SITE_URL } from "@/lib/config";
import { challengeS256, generateState, generateVerifier } from "@/lib/pkce";
import { safeNext } from "@/lib/redirect";

export const dynamic = "force-dynamic";

export async function GET(req: NextRequest) {
  if (!GAVAAUTH_URL) return NextResponse.redirect(`${SITE_URL}/login?error=config`);

  const next = safeNext(req.nextUrl.searchParams.get("next"));
  const verifier = generateVerifier();
  const state = generateState();

  const url = new URL(`${GAVAAUTH_URL}/login`);
  url.searchParams.set("client_id", GAVAAUTH_CLIENT_ID);
  url.searchParams.set("redirect_uri", REDIRECT_URI);
  url.searchParams.set("response_type", "code");
  url.searchParams.set("code_challenge", challengeS256(verifier));
  url.searchParams.set("code_challenge_method", "S256");
  url.searchParams.set("state", state);

  const res = NextResponse.redirect(url.toString());
  res.cookies.set(PKCE_COOKIE, JSON.stringify({ verifier, state, next }), {
    httpOnly: true,
    sameSite: "lax",
    secure: SECURE_COOKIES,
    path: "/auth",
    maxAge: 600,
  });
  return res;
}
