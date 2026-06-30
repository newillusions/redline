/**
 * Unit tests for the pure MRU list helpers in recent-docs.ts.
 *
 * Covers: insert-new, dedup-moves-to-top, cap-eviction, page-count storage.
 * IPC wrappers (loadRecentDocs / saveRecentDocs / checkFileExists) are tested
 * manually — they wrap Tauri invoke calls that cannot run in jsdom/vitest.
 */
import { describe, it, expect } from "vitest";
import { upsertMru, MAX_RECENT } from "./recent-docs";
import type { RecentDoc } from "./recent-docs";

function makeEntry(path: string, pageCount?: number): RecentDoc {
  return {
    path,
    file_name: path.split("/").at(-1) ?? path,
    last_opened: new Date().toISOString(),
    page_count: pageCount,
  };
}

describe("upsertMru", () => {
  it("prepends a new entry to an empty list", () => {
    const list: RecentDoc[] = [];
    const result = upsertMru(list, makeEntry("/a.pdf"));
    expect(result[0].path).toBe("/a.pdf");
    expect(result.length).toBe(1);
  });

  it("prepends a new entry to an existing list", () => {
    const list = [makeEntry("/a.pdf"), makeEntry("/b.pdf")];
    const result = upsertMru(list, makeEntry("/c.pdf"));
    expect(result[0].path).toBe("/c.pdf");
    expect(result.length).toBe(3);
  });

  it("moves an existing path to the top without duplicating", () => {
    const list = [makeEntry("/a.pdf"), makeEntry("/b.pdf"), makeEntry("/c.pdf")];
    const result = upsertMru(list, makeEntry("/b.pdf"));
    expect(result[0].path).toBe("/b.pdf");
    expect(result.length).toBe(3);
    expect(result.filter((e) => e.path === "/b.pdf").length).toBe(1);
  });

  it("moving the already-first entry keeps length unchanged", () => {
    const list = [makeEntry("/a.pdf"), makeEntry("/b.pdf")];
    const result = upsertMru(list, makeEntry("/a.pdf"));
    expect(result[0].path).toBe("/a.pdf");
    expect(result.length).toBe(2);
  });

  it("evicts oldest entries when cap is reached", () => {
    let list: RecentDoc[] = [];
    const cap = 3;
    for (let i = 0; i < 5; i++) {
      list = upsertMru(list, makeEntry(`/${i}.pdf`), cap);
    }
    expect(list.length).toBe(cap);
    expect(list[0].path).toBe("/4.pdf");
    expect(list[1].path).toBe("/3.pdf");
    expect(list[2].path).toBe("/2.pdf");
    expect(list.some((e) => e.path === "/0.pdf")).toBe(false);
    expect(list.some((e) => e.path === "/1.pdf")).toBe(false);
  });

  it("respects MAX_RECENT default cap", () => {
    let list: RecentDoc[] = [];
    for (let i = 0; i < MAX_RECENT + 5; i++) {
      list = upsertMru(list, makeEntry(`/${i}.pdf`));
    }
    expect(list.length).toBe(MAX_RECENT);
  });

  it("stores page count on the entry", () => {
    const list: RecentDoc[] = [];
    const result = upsertMru(list, makeEntry("/plan.pdf", 42));
    expect(result[0].page_count).toBe(42);
  });

  it("does not mutate the input list", () => {
    const original = [makeEntry("/a.pdf")];
    const frozen = [...original];
    upsertMru(original, makeEntry("/b.pdf"));
    // upsertMru returns a new array; original is not modified
    expect(original.length).toBe(frozen.length);
  });

  it("updates page_count when reopening an existing entry", () => {
    const list = [makeEntry("/a.pdf", 10)];
    const result = upsertMru(list, makeEntry("/a.pdf", 20));
    expect(result[0].page_count).toBe(20);
  });
});
