export function safeNext(next: string | null | undefined): string {
  if (!next || !next.startsWith("/")) return "/";
  if (next.startsWith("//") || next.startsWith("/\\")) return "/";

  for (const ch of next) {
    const c = ch.charCodeAt(0);
    if (c < 0x20 || c === 0x7f || ch === "\\") return "/";
  }
  return next;
}
