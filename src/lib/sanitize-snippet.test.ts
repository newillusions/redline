// @vitest-environment jsdom
import { describe, it, expect, vi } from "vitest";
import { sanitizeSnippetHtml } from "./sanitize-snippet";
import { SearchStore } from "./search-store.svelte";
import type { FolderSearchHit } from "./ipc";

const HOSTILE = [
  "<img src=x onerror=alert(1)>",
  "<script>alert(1)</script>",
  "<b onclick=alert(1)>x</b>",
  "<mark style=x>y</mark>",
  "<svg/onload=alert(1)>",
  "<b<img src=x onerror=alert(1)>>",
];

function assertInert(html: string) {
  const el = document.createElement("div");
  el.innerHTML = html;
  expect(el.querySelector("img, script, svg, iframe")).toBeNull();
  for (const n of Array.from(el.querySelectorAll("*"))) {
    expect(["B", "MARK"]).toContain(n.tagName);
    expect(n.attributes.length).toBe(0);
  }
}

describe("sanitizeSnippetHtml", () => {
  it("keeps bare <b>/<mark> tags and text", () => {
    expect(sanitizeSnippetHtml("a <b>hit</b> &amp; <mark>m</mark>")).toBe(
      "a <b>hit</b> &amp; <mark>m</mark>",
    );
  });

  it("neutralises hostile strings", () => {
    for (const h of HOSTILE) assertInert(sanitizeSnippetHtml(h));
  });
});

describe("search paths feed the sanitiser safely", () => {
  it.each(["searchFolder", "searchPaths"] as const)(
    "%s hostile hit renders inert",
    async (path) => {
      const hits: FolderSearchHit[] = HOSTILE.map((s, i) => ({
        file_path: `/p/${i}.pdf`,
        page_number: 1,
        snippet: s,
        source: "lopdf",
      }));
      const deps = {
        searchFolder: vi.fn(async () => hits),
        searchPaths: vi.fn(async () => hits),
      } as unknown as ConstructorParameters<typeof SearchStore>[0];
      const store = new SearchStore(deps);
      store.query = "x";
      await store.run(
        path === "searchFolder"
          ? { scope: "folder" }
          : { scope: "recents", paths: ["/p/0.pdf"] },
      );
      const all = store.groups.flatMap((g) => g.hits);
      expect(all.length).toBe(HOSTILE.length);
      for (const h of all)
        assertInert(h.snippetHtml ? sanitizeSnippetHtml(h.snippet) : "");
    },
  );
});
