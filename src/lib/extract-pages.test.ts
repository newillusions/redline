import { describe, it, expect } from "vitest";
import { sortedUniquePageIndices, formatPageRanges, suggestExtractedPath } from "./extract-pages";

describe("sortedUniquePageIndices", () => {
  it("sorts ascending", () => {
    expect(sortedUniquePageIndices([3, 1, 2])).toEqual([1, 2, 3]);
  });

  it("dedupes", () => {
    expect(sortedUniquePageIndices([1, 1, 2, 2, 3])).toEqual([1, 2, 3]);
  });

  it("handles an out-of-order ctrl-click selection", () => {
    expect(sortedUniquePageIndices([4, 0, 2])).toEqual([0, 2, 4]);
  });

  it("handles an empty input", () => {
    expect(sortedUniquePageIndices([])).toEqual([]);
  });
});

describe("formatPageRanges", () => {
  it("formats a single page", () => {
    expect(formatPageRanges([0])).toBe("1");
  });

  it("formats a contiguous range", () => {
    expect(formatPageRanges([0, 1, 2])).toBe("1-3");
  });

  it("formats scattered non-adjacent pages", () => {
    expect(formatPageRanges([0, 2, 4])).toBe("1, 3, 5");
  });

  it("formats a mix of ranges and singles", () => {
    expect(formatPageRanges([0, 1, 3, 4, 5, 8])).toBe("1-2, 4-6, 9");
  });

  it("sorts and dedupes before formatting (click order / duplicates don't matter)", () => {
    expect(formatPageRanges([2, 0, 1, 1])).toBe("1-3");
  });

  it("returns an empty string for no pages", () => {
    expect(formatPageRanges([])).toBe("");
  });

  it("formats a two-page range as N-M, not N, M", () => {
    expect(formatPageRanges([4, 5])).toBe("5-6");
  });
});

describe("suggestExtractedPath", () => {
  it("inserts -extracted before the extension, POSIX path", () => {
    expect(suggestExtractedPath("/a/b/plan.pdf")).toBe("/a/b/plan-extracted.pdf");
  });

  it("inserts -extracted before the extension, Windows path", () => {
    expect(suggestExtractedPath("C:\\Users\\m\\plan.pdf")).toBe("C:\\Users\\m\\plan-extracted.pdf");
  });

  it("handles a source with no directory", () => {
    expect(suggestExtractedPath("plan.pdf")).toBe("plan-extracted.pdf");
  });

  it("handles a source with no extension", () => {
    expect(suggestExtractedPath("/a/b/plan")).toBe("/a/b/plan-extracted.pdf");
  });

  it("handles a dotfile-style name without treating the leading dot as an extension", () => {
    expect(suggestExtractedPath("/a/.hidden")).toBe("/a/.hidden-extracted.pdf");
  });
});
