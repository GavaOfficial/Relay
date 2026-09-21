import { describe, expect, it } from "vitest";
import { clamp, commonEnd, driftAction, formatTime, livePosition, longestEnd } from "./sync";

describe("driftAction", () => {
  it("non fa nulla sotto soglia", () => {
    expect(driftAction(10, 10.05)).toEqual({ seekTo: null, rate: 1 });
    expect(driftAction(10, 9.95)).toEqual({ seekTo: null, rate: 1 });
  });
  it("rallenta o accelera per derive piccole", () => {
    expect(driftAction(10, 10.2)).toEqual({ seekTo: null, rate: 0.95 });
    expect(driftAction(10, 9.8)).toEqual({ seekTo: null, rate: 1.05 });
  });
  it("riallinea di colpo oltre 0,3 s", () => {
    expect(driftAction(10, 10.5)).toEqual({ seekTo: 10, rate: 1 });
    expect(driftAction(10, 9)).toEqual({ seekTo: 10, rate: 1 });
  });
});

describe("commonEnd", () => {
  it("prende il minimo dei valori finiti", () => {
    expect(commonEnd([30, 22.5, 40])).toBe(22.5);
    expect(commonEnd([30, null, Infinity, NaN, 25])).toBe(25);
  });
  it("e' null senza dati", () => {
    expect(commonEnd([])).toBeNull();
    expect(commonEnd([null, undefined])).toBeNull();
  });
});

describe("livePosition / clamp", () => {
  it("resta a 0 per registrazioni corte", () => {
    expect(livePosition(4)).toBe(0);
    expect(livePosition(60)).toBe(50);
    expect(livePosition(60, 6)).toBe(54);
  });
  it("clamp", () => {
    expect(clamp(-1, 0, 10)).toBe(0);
    expect(clamp(11, 0, 10)).toBe(10);
    expect(clamp(5, 0, 10)).toBe(5);
  });
});

describe("formatTime", () => {
  it("formatta m:ss e h:mm:ss", () => {
    expect(formatTime(0)).toBe("0:00");
    expect(formatTime(65.9)).toBe("1:05");
    expect(formatTime(3725)).toBe("1:02:05");
    expect(formatTime(NaN)).toBe("0:00");
    expect(formatTime(-3)).toBe("0:00");
  });
});

describe("longestEnd", () => {
  it("nel replay la fine e' quella della visuale piu' lunga", () => {
    expect(longestEnd([30, 22.5, 40])).toBe(40);
    expect(longestEnd([null, NaN, 12])).toBe(12);
    expect(longestEnd([])).toBeNull();
  });
});
