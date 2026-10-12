/**
 * Review read model for a markup: what a Markups List row needs (reply count, current
 * status, who set it and when, latest reply) derived from `workflow`. Pure; no DOM.
 * Mirrors Rust `Workflow::reply_count` / `latest_review_change`.
 */
import type { Markup, MarkupReply, MarkupStatus, StatusChange } from "./ipc";

export interface ReviewSummary {
  replyCount: number;
  status: MarkupStatus;
  /** Who set the current status and when; null when no Review-model change is recorded. */
  statusBy: string | null;
  statusAt: string | null;
  lastReply: MarkupReply | null;
  /** The replies in display order with their nesting depth (0 = direct reply to the markup). */
  thread: { reply: MarkupReply; depth: number }[];
}

function isReview(c: StatusChange): boolean {
  return c.model === "Review" && !c.on_reply;
}

/** Latest markup-level Review-model entry of a history (oldest-first array), or null. */
export function latestReviewChange(history: StatusChange[] | undefined): StatusChange | null {
  const h = history ?? [];
  for (let i = h.length - 1; i >= 0; i--) if (isReview(h[i])) return h[i];
  return null;
}

/** Replies depth-first under their parent, siblings oldest first (Revu's indented rows). */
export function threadRows(thread: MarkupReply[]): { reply: MarkupReply; depth: number }[] {
  const ids = new Set(thread.map((r) => r.id));
  const children = new Map<string | null, MarkupReply[]>();
  for (const r of thread) {
    const key = r.in_reply_to && ids.has(r.in_reply_to) ? r.in_reply_to : null;
    children.set(key, [...(children.get(key) ?? []), r]);
  }
  const out: { reply: MarkupReply; depth: number }[] = [];
  const seen = new Set<string>();
  const walk = (parent: string | null, depth: number) => {
    for (const r of children.get(parent) ?? []) {
      if (seen.has(r.id)) continue;
      seen.add(r.id);
      out.push({ reply: r, depth });
      walk(r.id, depth + 1);
    }
  };
  walk(null, 0);
  return out;
}

export function reviewSummary(m: Pick<Markup, "workflow">): ReviewSummary {
  const w = m.workflow;
  const change = latestReviewChange(w.status_history);
  const rows = threadRows(w.thread);
  const byTime = [...w.thread].sort((a, b) => a.at.localeCompare(b.at));
  return {
    replyCount: w.thread.length,
    status: w.status,
    statusBy: change?.author.display_name ?? null,
    statusAt: change?.at ?? null,
    lastReply: byTime.length ? byTime[byTime.length - 1] : null,
    thread: rows,
  };
}
