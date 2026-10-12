<script lang="ts">
  /**
   * LinkOverlay - the Link layer on top of a page in the viewport (docs/navigation.md
   * RH1-RH3). Three modes:
   *   - "off":    nothing is drawn or captured.
   *   - "follow": (Hand tool) each link on the page is a clickable region; the outline shows
   *               on hover. A click that is really a drag (panning) does nothing.
   *   - "draw":   (Link tool) every link is outlined in blue and a drag draws a new link
   *               area; Esc leaves. The overlay captures the pointer, so the Hand tool does
   *               not pan underneath.
   * Geometry is in the viewer's page space; this component only maps it to the screen.
   */
  import type { LinkView } from "$lib/navigation-ipc";
  import { pdfUserSpaceToScreen, screenToPdfUserSpace, type ViewportState } from "$lib/viewport";
  import { rectFromCorners, rectIsUsable, targetText } from "$lib/navigation";

  const {
    links,
    viewState,
    mode,
    labels = [],
    onfollow,
    oncreate,
    oncancel,
  }: {
    links: readonly LinkView[];
    viewState: ViewportState;
    mode: "off" | "follow" | "draw";
    labels?: readonly string[];
    onfollow: (l: LinkView) => void;
    oncreate: (rect: [number, number, number, number]) => void;
    oncancel: () => void;
  } = $props();

  let svgEl = $state<SVGSVGElement | null>(null);
  let drag = $state<{ x0: number; y0: number; x1: number; y1: number } | null>(null);
  let downAt: { x: number; y: number } | null = null;

  const boxes = $derived(
    links.map((l) => {
      const a = pdfUserSpaceToScreen(l.rect[0], l.rect[3], viewState);
      const b = pdfUserSpaceToScreen(l.rect[2], l.rect[1], viewState);
      return {
        link: l,
        x: Math.min(a.x, b.x),
        y: Math.min(a.y, b.y),
        w: Math.abs(b.x - a.x),
        h: Math.abs(b.y - a.y),
      };
    }),
  );

  function local(e: PointerEvent): { x: number; y: number } {
    const r = svgEl?.getBoundingClientRect();
    return { x: e.clientX - (r?.left ?? 0), y: e.clientY - (r?.top ?? 0) };
  }

  function onPointerDown(e: PointerEvent) {
    if (mode !== "draw" || e.button !== 0) return;
    e.preventDefault();
    const p = local(e);
    drag = { x0: p.x, y0: p.y, x1: p.x, y1: p.y };
    svgEl?.setPointerCapture?.(e.pointerId);
  }

  function onPointerMove(e: PointerEvent) {
    if (!drag) return;
    const p = local(e);
    drag = { ...drag, x1: p.x, y1: p.y };
  }

  function onPointerUp(e: PointerEvent) {
    if (!drag) return;
    const d = drag;
    drag = null;
    svgEl?.releasePointerCapture?.(e.pointerId);
    const a = screenToPdfUserSpace(d.x0, d.y0, viewState);
    const b = screenToPdfUserSpace(d.x1, d.y1, viewState);
    const rect = rectFromCorners(a, b);
    if (rectIsUsable(rect)) oncreate(rect);
  }

  function onLinkDown(e: PointerEvent) {
    downAt = { x: e.clientX, y: e.clientY };
  }

  function onLinkClick(e: MouseEvent, l: LinkView) {
    if (e.button !== 0) return;
    // A drag that happened to end over a link is a pan, not a click.
    if (downAt && Math.hypot(e.clientX - downAt.x, e.clientY - downAt.y) > 4) return;
    onfollow(l);
  }

  function onKeyDown(e: KeyboardEvent) {
    if (mode === "draw" && e.key === "Escape") {
      e.preventDefault();
      drag = null;
      oncancel();
    }
  }
</script>

<svelte:window onkeydown={onKeyDown} />

{#if mode !== "off"}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <svg
    bind:this={svgEl}
    class="link-overlay"
    class:draw={mode === "draw"}
    aria-hidden={mode === "draw" ? undefined : true}
    onpointerdown={onPointerDown}
    onpointermove={onPointerMove}
    onpointerup={onPointerUp}
    onpointercancel={() => (drag = null)}
    onmousedown={(e) => {
      if (mode === "draw") e.stopPropagation();
    }}
  >
    {#each boxes as b (b.link.id ?? `${b.link.page}:${b.x}:${b.y}`)}
      {#if mode === "draw"}
        <rect class="link-box" x={b.x} y={b.y} width={b.w} height={b.h} pointer-events="none" />
      {:else}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <rect
          class="link-hit"
          x={b.x}
          y={b.y}
          width={b.w}
          height={b.h}
          pointer-events="all"
          role="link"
          tabindex="-1"
          aria-label={targetText(b.link.target, labels)}
          onpointerdown={onLinkDown}
          onclick={(e) => onLinkClick(e, b.link)}
        >
          <title>{targetText(b.link.target, labels)}</title>
        </rect>
      {/if}
    {/each}
    {#if drag}
      <rect
        class="link-drag"
        x={Math.min(drag.x0, drag.x1)}
        y={Math.min(drag.y0, drag.y1)}
        width={Math.abs(drag.x1 - drag.x0)}
        height={Math.abs(drag.y1 - drag.y0)}
        pointer-events="none"
      />
    {/if}
  </svg>
{/if}

<style>
  .link-overlay {
    position: absolute;
    top: 0;
    left: 0;
    width: 100%;
    height: 100%;
    pointer-events: none;
  }
  .link-overlay.draw {
    pointer-events: auto;
    cursor: crosshair;
  }
  .link-box,
  .link-drag {
    fill: rgba(10, 132, 255, 0.12);
    stroke: var(--color-primary);
    stroke-width: 1.5;
  }
  .link-drag {
    stroke-dasharray: 4 3;
  }
  .link-hit {
    fill: transparent;
    stroke: transparent;
    stroke-width: 1.5;
    cursor: pointer;
  }
  .link-hit:hover {
    fill: rgba(10, 132, 255, 0.1);
    stroke: var(--color-primary);
  }
</style>
