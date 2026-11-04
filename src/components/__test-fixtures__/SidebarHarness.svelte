<script lang="ts">
  // Test-only host for Sidebar.svelte: content snippets have to be declared in a component.
  import Sidebar from "../Sidebar.svelte";
  import type { PanelLayoutStore } from "$lib/panel-layout-store.svelte";
  import type { PanelId } from "$lib/panel-layout";

  interface Props {
    store: PanelLayoutStore;
    searchOpen?: boolean;
    onclosesearch?: () => void;
  }
  const { store, searchOpen = false, onclosesearch = () => {} }: Props = $props();
</script>

{#snippet search()}<p>search body</p>{/snippet}
{#snippet recent()}<p>recent body</p>{/snippet}
{#snippet toolchest()}<ul>{#each Array.from({ length: 50 }, (_, i) => i) as i (i)}<li>tool {i}</li>{/each}</ul>{/snippet}
{#snippet layers()}<p>layers body</p>{/snippet}
{#snippet properties()}<p>properties body</p>{/snippet}

{#each ["left", "right"] as const as side (side)}
  <Sidebar
    {side}
    {store}
    content={{ search, recent, toolchest, layers, properties }}
    shown={(id: PanelId) => id !== "search" || searchOpen}
    closeActions={{ search: onclosesearch }}
    testId="sidebar-{side}"
  />
{/each}
