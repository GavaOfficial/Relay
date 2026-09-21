"use server";

import { cookies } from "next/headers";
import { redirect } from "next/navigation";
import { API_ORIGIN, SESSION_COOKIE } from "@/lib/api";
import { DEV_LOGIN, GAVAAUTH_URL, SECURE_COOKIES, SITE_URL } from "@/lib/config";
import { safeNext } from "@/lib/redirect";

export async function login(formData: FormData) {
  const next = safeNext(String(formData.get("next") ?? ""));
  const back = (e: string) => `/login?error=${e}${next !== "/" ? `&next=${encodeURIComponent(next)}` : ""}`;
  if (!DEV_LOGIN) redirect(back("config"));

  const token = String(formData.get("token") ?? "").trim();
  if (!token) redirect(back("1"));

  let ok = false;
  try {
    const r = await fetch(`${API_ORIGIN}/api/me`, {
      headers: { authorization: `Bearer ${token}` },
      cache: "no-store",
    });
    ok = r.ok;
  } catch {
    redirect(back("server"));
  }
  if (!ok) redirect(back("1"));

  (await cookies()).set(SESSION_COOKIE, token, {
    httpOnly: true,
    sameSite: "lax",
    secure: SECURE_COOKIES,
    path: "/",
    maxAge: 60 * 60 * 24 * 30,
  });
  redirect(next);
}

export async function logout() {
  (await cookies()).delete(SESSION_COOKIE);
  if (GAVAAUTH_URL) {
    const back = encodeURIComponent(`${SITE_URL}/login`);
    redirect(`${GAVAAUTH_URL}/logout?post_logout_redirect_uri=${back}`);
  }
  redirect("/login");
}
