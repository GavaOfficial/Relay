import { describe, expect, it } from "vitest";
import { challengeS256, generateState, generateVerifier, safeEqual } from "./pkce";

describe("pkce", () => {
  it("challenge: vettore RFC 7636 appendice B", () => {
    expect(challengeS256("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk")).toBe(
      "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
    );
  });
  it("verifier: 43 caratteri base64url, diversi a ogni chiamata", () => {
    const a = generateVerifier();
    expect(a).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(generateVerifier()).not.toBe(a);
  });
  it("state casuale", () => {
    expect(generateState()).toMatch(/^[A-Za-z0-9_-]{20,}$/);
    expect(generateState()).not.toBe(generateState());
  });
  it("safeEqual", () => {
    expect(safeEqual("abc", "abc")).toBe(true);
    expect(safeEqual("abc", "abd")).toBe(false);
    expect(safeEqual("abc", "abcd")).toBe(false);
  });
});
