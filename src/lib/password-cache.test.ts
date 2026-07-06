import { describe, it, expect } from "vitest";
import {
  createPasswordCache,
  getCachedPassword,
  setCachedPassword,
  clearCachedPassword,
} from "./password-cache";

describe("password-cache", () => {
  it("returns undefined for a path never cached", () => {
    const cache = createPasswordCache();
    expect(getCachedPassword(cache, "/tmp/a.pdf")).toBeUndefined();
  });

  it("returns the password after it is set", () => {
    const cache = createPasswordCache();
    setCachedPassword(cache, "/tmp/a.pdf", "secret");
    expect(getCachedPassword(cache, "/tmp/a.pdf")).toBe("secret");
  });

  it("keeps separate entries per path", () => {
    const cache = createPasswordCache();
    setCachedPassword(cache, "/tmp/a.pdf", "secret-a");
    setCachedPassword(cache, "/tmp/b.pdf", "secret-b");
    expect(getCachedPassword(cache, "/tmp/a.pdf")).toBe("secret-a");
    expect(getCachedPassword(cache, "/tmp/b.pdf")).toBe("secret-b");
  });

  it("overwrites the password when set again for the same path", () => {
    const cache = createPasswordCache();
    setCachedPassword(cache, "/tmp/a.pdf", "old");
    setCachedPassword(cache, "/tmp/a.pdf", "new");
    expect(getCachedPassword(cache, "/tmp/a.pdf")).toBe("new");
  });

  it("clears a cached password", () => {
    const cache = createPasswordCache();
    setCachedPassword(cache, "/tmp/a.pdf", "secret");
    clearCachedPassword(cache, "/tmp/a.pdf");
    expect(getCachedPassword(cache, "/tmp/a.pdf")).toBeUndefined();
  });

  it("clearing an uncached path is a no-op", () => {
    const cache = createPasswordCache();
    expect(() => clearCachedPassword(cache, "/tmp/nope.pdf")).not.toThrow();
  });
});
