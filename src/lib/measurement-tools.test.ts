import { describe, it, expect } from "vitest";
import {
  measureLength,
  measureArea,
  formatQuantity,
} from "./measurement-tools";
import type { ScaleRecord } from "./ipc";

function scale(ratio: number, unit = "m", precision = 2): ScaleRecord {
  return {
    id: "sc1",
    applies_to: { kind: "DocumentDefault" },
    method: "Preset",
    ratio,
    unit,
    label: "1:1000",
    precision,
  };
}

describe("measureLength", () => {
  it("returns 0 for a single point", () => {
    expect(measureLength([{ x: 0, y: 0 }])).toBe(0);
  });

  it("computes 3-4-5 hypotenuse", () => {
    expect(measureLength([{ x: 0, y: 0 }, { x: 3, y: 4 }])).toBeCloseTo(5, 9);
  });

  it("sums two segments", () => {
    const pts = [{ x: 0, y: 0 }, { x: 1, y: 0 }, { x: 1, y: 1 }];
    expect(measureLength(pts)).toBeCloseTo(2, 9);
  });
});

describe("measureArea", () => {
  it("returns 0 for fewer than 3 points", () => {
    expect(measureArea([{ x: 0, y: 0 }, { x: 1, y: 0 }])).toBe(0);
  });

  it("computes unit square area", () => {
    const pts = [{ x: 0, y: 0 }, { x: 1, y: 0 }, { x: 1, y: 1 }, { x: 0, y: 1 }];
    expect(measureArea(pts)).toBeCloseTo(1, 9);
  });

  it("computes right triangle area", () => {
    const pts = [{ x: 0, y: 0 }, { x: 6, y: 0 }, { x: 0, y: 4 }];
    expect(measureArea(pts)).toBeCloseTo(12, 9);
  });
});

describe("formatQuantity", () => {
  it("formats a length with the scale unit", () => {
    const s = scale(0.001, "m", 2);
    // raw 5000 pts × 0.001 = 5.00 m
    expect(formatQuantity(5000, s, false)).toBe("5.00 m");
  });

  it("formats an area (ratio²) with unit²", () => {
    const s = scale(0.001, "m", 2);
    // raw 1_000_000 pts² × 0.001² = 1.00 m²
    expect(formatQuantity(1_000_000, s, true)).toBe("1.00 m²");
  });

  it("respects precision 0", () => {
    const s = scale(1 / 100, "cm", 0);
    expect(formatQuantity(300, s, false)).toBe("3 cm");
  });
});
