import { NextResponse, type NextRequest } from "next/server";
import { SESSION_COOKIE } from "@/lib/api";

export function GET(req: NextRequest) {
  const token = process.env.RELAY_DEV_SESSION;
  if (process.env.NODE_ENV !== "development" || !token) {
    return new NextResponse("Not found", { status: 404 });
  }
  const next = req.nextUrl.searchParams.get("next") ?? "/";
  const res = NextResponse.redirect(new URL(next.startsWith("/") ? next : "/", req.url));
  res.cookies.set(SESSION_COOKIE, token, { httpOnly: true, sameSite: "lax", path: "/" });
  return res;
}
