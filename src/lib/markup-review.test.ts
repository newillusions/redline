import { describe, it, expect } from "vitest";
import { reviewSummary, threadRows, latestReviewChange } from "./markup-review";
import type { MarkupReply, StatusChange, UserRef } from "./ipc";

const A: UserRef = { user_id: "11111111-1111-1111-1111-111111111111", display_name: "Architect" };
const M: UserRef = { user_id: "22222222-2222-2222-2222-222222222222", display_name: "Martin" };
const reply = (id: string, at: string, parent: string | null, by = A): MarkupReply => ({
  id,
  author: by,
  at,
  contents: id,
  in_reply_to: parent,
});
const change = (id: string, at: string, model: string, state: string, by = A): StatusChange => ({
  id,
  author: by,
  at,
  model,
  state,
});

describe("reviewSummary", () => {
  it("reports reply count, current status, who set it and when, and the latest reply", () => {
    const s = reviewSummary({
      workflow: {
        status: "Completed",
        assignee: null,
        thread: [reply("a", "2026-10-01T09:10:00Z", null), reply("b", "2026-10-01T09:20:00Z", "a", M)],
        status_history: [
          change("1", "2026-10-01T09:30:00Z", "Review", "Accepted"),
          change("2", "2026-10-01T09:40:00Z", "Review", "Completed", M),
          change("3", "2026-10-01T09:50:00Z", "Marked", "Marked", M),
        ],
      },
    });
    expect(s.replyCount).toBe(2);
    expect(s.status).toBe("Completed");
    expect(s.statusBy).toBe("Martin");
    expect(s.statusAt).toBe("2026-10-01T09:40:00Z");
    expect(s.lastReply?.id).toBe("b");
  });

  it("has no status author when only the legacy status exists", () => {
    const s = reviewSummary({ workflow: { status: "Accepted", assignee: null, thread: [] } });
    expect(s.statusBy).toBeNull();
    expect(s.lastReply).toBeNull();
  });
});

describe("threadRows", () => {
  it("nests replies under their parent depth-first", () => {
    const rows = threadRows([
      reply("a", "1", null),
      reply("b", "2", "a"),
      reply("c", "3", null),
      reply("d", "4", "b"),
    ]);
    expect(rows.map((r) => [r.reply.id, r.depth])).toEqual([
      ["a", 0],
      ["b", 1],
      ["d", 2],
      ["c", 0],
    ]);
  });

  it("treats a reply to an unknown parent as direct", () => {
    expect(threadRows([reply("x", "1", "missing")])[0].depth).toBe(0);
  });

  it("survives a cycle", () => {
    expect(threadRows([reply("a", "1", "b"), reply("b", "2", "a")]).length).toBeLessThanOrEqual(2);
  });
});

describe("latestReviewChange", () => {
  it("ignores a state set on a reply", () => {
    const onReply = { ...change("1", "2026-10-01T09:00:00Z", "Review", "Rejected"), on_reply: "r1" };
    expect(latestReviewChange([onReply])).toBeNull();
  });

  it("ignores non-Review models", () => {
    expect(latestReviewChange([change("1", "t", "Marked", "Marked")])).toBeNull();
  });
});
