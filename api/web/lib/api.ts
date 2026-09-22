import "server-only";
import { cookies } from "next/headers";
import { notFound, redirect } from "next/navigation";

export const API_ORIGIN = process.env.API_ORIGIN ?? "http://127.0.0.1:8080";

export const SESSION_COOKIE = "relay_token";

export async function sessionToken(): Promise<string | undefined> {
  return (await cookies()).get(SESSION_COOKIE)?.value;
}

export async function apiGet<T>(path: string): Promise<T> {
  const token = await sessionToken();
  if (!token) redirect("/login");
  const r = await fetch(`${API_ORIGIN}${path}`, {
    headers: { authorization: `Bearer ${token}` },
    cache: "no-store",
  });
  if (r.status === 401) redirect("/login");
  if (r.status === 403 || r.status === 404) notFound();
  if (!r.ok) throw new Error(`API ${path}: ${r.status}`);
  return (await r.json()) as T;
}

export type AppRelease = { version: string; size: number; notes: string };

export async function latestApp(): Promise<AppRelease | null> {
  try {
    const r = await fetch(`${API_ORIGIN}/api/app/latest`, { next: { revalidate: 300 } });
    if (!r.ok) return null;
    const j = (await r.json()) as AppRelease;
    return typeof j.version === "string" ? j : null;
  } catch {
    return null;
  }
}

export async function currentIdentity(): Promise<{ user: string; name: string | null } | null> {
  const token = await sessionToken();
  if (!token) return null;
  try {
    const r = await fetch(`${API_ORIGIN}/api/me`, {
      headers: { authorization: `Bearer ${token}` },
      cache: "no-store",
    });
    if (!r.ok) return null;
    const j = (await r.json()) as { user: string; name?: string | null };
    return { user: j.user, name: j.name ?? null };
  } catch {
    return null;
  }
}

export async function currentUser(): Promise<string | null> {
  return (await currentIdentity())?.user ?? null;
}

export async function apiGetShare<T>(path: string): Promise<T | null> {
  try {
    const r = await fetch(`${API_ORIGIN}${path}`, { cache: "no-store" });
    if (!r.ok) return null;
    return (await r.json()) as T;
  } catch {
    return null;
  }
}
