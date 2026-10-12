/**
 * Pure property-patch and indeterminate-value helpers for the PropertiesPanel.
 * No DOM, no Svelte, no side effects. The panel component stays thin by delegating
 * all mutation logic here.
 */
import type { Markup, Appearance, UserRef, MarkupWorkflow, MarkupReply, StatusChange } from "./ipc";
import { bumpAudit, DEFAULT_TEXT_FONT } from "./markup-tools";
import { refitBox, setAutosize } from "./callout";

/** Base-14 font families offered by the picker (reliable cross-viewer /DA rendering). */
export const FONT_FAMILIES: readonly string[] = ["Helvetica", "Times", "Courier"];

/** Font sizes (pt) offered by the picker. */
export const FONT_SIZES: readonly number[] = [8, 9, 10, 11, 12, 14, 18, 24, 36];

/**
 * Return a clone of `m` with a shallow appearance patch applied and the audit
 * trail bumped. The font field can be set via `patch = { font: {family, size_pt} }`
 * or cleared with `patch = { font: null }`. No mutation of the input.
 */
export function patchAppearance(
  m: Markup,
  patch: Partial<Appearance>,
  by: UserRef,
  now: string,
): Markup {
  const patched: Markup = { ...m, appearance: { ...m.appearance, ...patch } };
  return bumpAudit(refitBox(patched, patched.appearance.font?.size_pt ?? DEFAULT_TEXT_FONT.size_pt), by, now);
}

/** Turn a Callout's text-box auto-size on (refitting the box) or off. Other types pass through. */
export function patchCalloutAutosize(m: Markup, on: boolean, by: UserRef, now: string): Markup {
  if (m.markup_type !== "Callout") return m;
  return bumpAudit(setAutosize(m, on, m.appearance.font?.size_pt ?? DEFAULT_TEXT_FONT.size_pt), by, now);
}

/**
 * Return a clone of `m` with only the supplied text fields applied and the audit
 * trail bumped. Keys absent from `patch` are left unchanged on the clone. An
 * explicit `null` value in `patch` does set the corresponding field to null.
 * No mutation of the input.
 */
export function patchFields(
  m: Markup,
  patch: {
    contents?: string | null;
    subject?: string | null;
    layer?: string | null;
    /** Assign to a layer (or none): the `/OC` reference; set together with `layer` (its name). */
    optional_content?: Markup["optional_content"];
  },
  by: UserRef,
  now: string,
): Markup {
  const updated: Markup = { ...m };
  if ("contents" in patch) updated.contents = patch.contents ?? null;
  if ("subject" in patch) updated.subject = patch.subject ?? null;
  if ("layer" in patch) updated.layer = patch.layer ?? null;
  if ("optional_content" in patch) updated.optional_content = patch.optional_content ?? null;
  const fitted = "contents" in patch
    ? refitBox(updated, updated.appearance.font?.size_pt ?? DEFAULT_TEXT_FONT.size_pt)
    : updated;
  return bumpAudit(fitted, by, now);
}

/**
 * Return a clone of `m` with `group_id` set to `groupId` and the audit trail bumped.
 * Pass `null` to ungroup. Pure; no mutation of the input.
 */
export function patchGroup(m: Markup, groupId: string | null, by: UserRef, now: string): Markup {
  return bumpAudit({ ...m, group_id: groupId }, by, now);
}

/** Fresh id for a reply / status entry (UUID, matches the Rust `Uuid` fields). */
function newId(): string {
  return crypto.randomUUID();
}

/**
 * Return a clone of `m` with `workflow.status` set to `status` and the audit trail
 * bumped. A change also appends a Review-model entry to `workflow.status_history`
 * (who/when), which the backend writes as a standard PDF state annotation so Bluebeam
 * Revu and Acrobat show the same status and its history. Setting the status it already
 * has records nothing. `assignee`/`thread` are left untouched. Pure; no mutation of the
 * input.
 */
export function patchStatus(
  m: Markup,
  status: MarkupWorkflow["status"],
  by: UserRef,
  now: string,
  id: string = newId(),
): Markup {
  const history = m.workflow.status_history ?? [];
  if (status === m.workflow.status) {
    return bumpAudit({ ...m, workflow: { ...m.workflow, status } }, by, now);
  }
  const change: StatusChange = { id, author: by, at: now, model: "Review", state: status };
  return bumpAudit(
    { ...m, workflow: { ...m.workflow, status, status_history: [...history, change] } },
    by,
    now,
  );
}

/**
 * Return a clone of `m` with a reply appended to its thread. `inReplyTo` is the id of the
 * reply being answered (null = a direct reply to the markup). Blank text is rejected by
 * returning `m` unchanged. Pure; no mutation of the input.
 */
export function patchAddReply(
  m: Markup,
  contents: string,
  by: UserRef,
  now: string,
  inReplyTo: string | null = null,
  id: string = newId(),
): Markup {
  const text = contents.trim();
  if (!text) return m;
  const reply: MarkupReply = { id, author: by, at: now, contents: text, in_reply_to: inReplyTo };
  return bumpAudit({ ...m, workflow: { ...m.workflow, thread: [...m.workflow.thread, reply] } }, by, now);
}

/**
 * Return a clone of `m` without reply `replyId` and the replies nested under it
 * (Revu's "Delete" on a reply). Unknown id returns `m` unchanged. Pure.
 */
export function patchDeleteReply(m: Markup, replyId: string, by: UserRef, now: string): Markup {
  const doomed = new Set<string>([replyId]);
  const history = m.workflow.status_history ?? [];
  // Replies nested under a doomed reply, and states set on a doomed reply or state, go too:
  // a state left behind would be orphaned (the backend drops it rather than re-anchor it on
  // the markup, where it would become the markup's own status).
  let grew = true;
  while (grew) {
    grew = false;
    for (const r of m.workflow.thread) {
      if (r.in_reply_to && doomed.has(r.in_reply_to) && !doomed.has(r.id)) {
        doomed.add(r.id);
        grew = true;
      }
    }
    for (const c of history) {
      if (c.on_reply && doomed.has(c.on_reply) && !doomed.has(c.id)) {
        doomed.add(c.id);
        grew = true;
      }
    }
  }
  const thread = m.workflow.thread.filter((r) => !doomed.has(r.id));
  if (thread.length === m.workflow.thread.length) return m;
  const status_history = history.filter((c) => !doomed.has(c.id));
  return bumpAudit({ ...m, workflow: { ...m.workflow, thread, status_history } }, by, now);
}

/** The standard PDF `/Text` icon names (ISO 32000-1 12.5.6.4), in menu order. */
export const NOTE_ICONS: readonly string[] = [
  "Note", "Comment", "Key", "Help", "NewParagraph", "Paragraph", "Insert",
];

/**
 * Return a clone of a Note markup with its icon (`/Name`) set to `icon` and the audit trail
 * bumped. Non-Note markups are returned unchanged (same reference). Pure.
 */
export function patchNoteIcon(m: Markup, icon: string, by: UserRef, now: string): Markup {
  if (m.markup_type !== "Note") return m;
  return bumpAudit({ ...m, note: { open: m.note?.open ?? false, icon } }, by, now);
}

/**
 * Return the shared projected value across all `markups` when every element
 * projects the same value under strict equality (===), or `undefined` when the
 * list is empty or the values differ.
 *
 * Use primitive projections (color strings, numbers, family strings) for
 * reliable equality; object projections will always return `undefined` unless
 * they share the same reference.
 */
export function commonValue<T>(markups: Markup[], get: (m: Markup) => T): T | undefined {
  if (markups.length === 0) return undefined;
  const first = get(markups[0]);
  for (let i = 1; i < markups.length; i++) {
    if (get(markups[i]) !== first) return undefined;
  }
  return first;
}
