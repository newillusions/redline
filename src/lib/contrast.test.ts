// @vitest-environment node
/** WCAG AA (4.5:1) assertions for token pairs the UI actually uses, read from styles.css. */
import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const css = readFileSync(resolve(__dirname, "styles.css"), "utf8");

function token(name: string): string {
  const m = css.match(new RegExp(`--${name}:\\s*(#[0-9a-fA-F]{6})`));
  if (!m) throw new Error(`token --${name} not found`);
  return m[1];
}

function luminance(hex: string): number {
  const c = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255)
    .map((x) => (x <= 0.03928 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4));
  return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
}

function ratio(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

describe("design token contrast (WCAG AA 4.5:1)", () => {
  it("--color-text on the filled primary-strong button background", () => {
    expect(ratio(token("color-text"), token("color-primary-strong"))).toBeGreaterThanOrEqual(4.5);
  });

  for (const bg of ["bg", "bg-panel", "bg-panel-alt", "bg-toolbar", "bg-hover", "bg-active"]) {
    it(`--color-text-muted on --color-${bg}`, () => {
      expect(ratio(token("color-text-muted"), token(`color-${bg}`))).toBeGreaterThanOrEqual(4.5);
    });
  }
});
