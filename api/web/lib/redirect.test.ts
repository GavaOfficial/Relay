import { describe, expect, it } from "vitest";
import { safeNext } from "./redirect";

describe("safeNext", () => {
  it("accetta percorsi relativi", () => {
    expect(safeNext("/")).toBe("/");
    expect(safeNext("/matches/abc")).toBe("/matches/abc");
    expect(safeNext("/join/x?code=y&z=1")).toBe("/join/x?code=y&z=1");
    expect(safeNext("/join/x%3Fcode")).toBe("/join/x%3Fcode");
  });
  it("rifiuta open redirect", () => {
    expect(safeNext("//evil.com")).toBe("/");
    expect(safeNext("/\\evil.com")).toBe("/");
    expect(safeNext("/a\\b")).toBe("/");
    expect(safeNext("https://x")).toBe("/");
    expect(safeNext("javascript:alert(1)")).toBe("/");
    expect(safeNext("evil.com")).toBe("/");
    expect(safeNext("/a\r\nb")).toBe("/");
  });
  it("vuoti", () => {
    expect(safeNext("")).toBe("/");
    expect(safeNext(null)).toBe("/");
    expect(safeNext(undefined)).toBe("/");
  });
});
