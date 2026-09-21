import { createHash, randomBytes, timingSafeEqual } from "node:crypto";

export function generateVerifier(): string {
  return randomBytes(32).toString("base64url");
}

export function challengeS256(verifier: string): string {
  return createHash("sha256").update(verifier).digest("base64url");
}

export function generateState(): string {
  return randomBytes(24).toString("base64url");
}

export function safeEqual(a: string, b: string): boolean {
  const ba = Buffer.from(a);
  const bb = Buffer.from(b);
  return ba.length === bb.length && timingSafeEqual(ba, bb);
}
