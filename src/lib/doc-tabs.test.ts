/**
 * Unit tests for DocTabStore — multi-document tab management logic.
 *
 * Covers: add/switch/close, dedup-open-same-path, close-active-activates-neighbor,
 * close-last->empty, per-tab viewport snapshot independence.
 */
import { describe, it, expect } from "vitest";
import { DocTabStore, DEFAULT_VIEWPORT_SNAPSHOT } from "./doc-tabs.svelte";
import { MarkupStore } from "./markup-store.svelte";
import { TakeoffStore } from "./takeoff-store.svelte";
import type { DocumentInfo } from "./ipc";

function makeDoc(id: string, path = `/docs/${id}.pdf`): DocumentInfo {
  return { doc_id: id, path, page_count: 5, was_encrypted: false };
}

function fakeIpc() {
  return {
    add: async () => {},
    update: async () => {},
    remove: async () => {},
  };
}

function makeTab(id: string, path?: string) {
  const doc = makeDoc(id, path);
  const store = new MarkupStore(id, fakeIpc());
  const ts = new TakeoffStore();
  return { doc, store, takeoffStore: ts };
}

describe("DocTabStore", () => {
  describe("addTab", () => {
    it("adds the first tab and makes it active", () => {
      const ts = new DocTabStore();
      const { doc, store, takeoffStore } = makeTab("doc1");
      ts.addTab(doc, store, takeoffStore);
      expect(ts.tabs.length).toBe(1);
      expect(ts.activeDocId).toBe("doc1");
    });

    it("adds a second tab and switches active to it", () => {
      const ts = new DocTabStore();
      const t1 = makeTab("doc1");
      const t2 = makeTab("doc2");
      ts.addTab(t1.doc, t1.store, t1.takeoffStore);
      ts.addTab(t2.doc, t2.store, t2.takeoffStore);
      expect(ts.tabs.length).toBe(2);
      expect(ts.activeDocId).toBe("doc2");
    });

    it("activeTab getter returns the current active tab", () => {
      const ts = new DocTabStore();
      const t1 = makeTab("doc1");
      ts.addTab(t1.doc, t1.store, t1.takeoffStore);
      expect(ts.activeTab?.docId).toBe("doc1");
      expect(ts.activeTab?.doc.path).toBe("/docs/doc1.pdf");
    });

    it("new tab starts with default viewport snapshot", () => {
      const ts = new DocTabStore();
      const t1 = makeTab("doc1");
      const tab = ts.addTab(t1.doc, t1.store, t1.takeoffStore);
      expect(tab.viewportSnapshot).toEqual(DEFAULT_VIEWPORT_SNAPSHOT);
    });

    it("isEncrypted defaults to false when omitted", () => {
      const ts = new DocTabStore();
      const t1 = makeTab("doc1");
      const tab = ts.addTab(t1.doc, t1.store, t1.takeoffStore);
      expect(tab.isEncrypted).toBe(false);
    });

    it("isEncrypted is true when a password-protected doc is opened", () => {
      const ts = new DocTabStore();
      const t1 = makeTab("doc1");
      const tab = ts.addTab(t1.doc, t1.store, t1.takeoffStore, true);
      expect(tab.isEncrypted).toBe(true);
    });
  });

  describe("switchTab", () => {
    it("switches the active tab to an existing tab", () => {
      const ts = new DocTabStore();
      const t1 = makeTab("doc1");
      const t2 = makeTab("doc2");
      ts.addTab(t1.doc, t1.store, t1.takeoffStore);
      ts.addTab(t2.doc, t2.store, t2.takeoffStore);
      ts.switchTab("doc1");
      expect(ts.activeDocId).toBe("doc1");
    });

    it("does nothing when switching to a non-existent tab", () => {
      const ts = new DocTabStore();
      const t1 = makeTab("doc1");
      ts.addTab(t1.doc, t1.store, t1.takeoffStore);
      ts.switchTab("nope");
      expect(ts.activeDocId).toBe("doc1");
    });
  });

  describe("findByPath (dedup guard)", () => {
    it("returns the matching tab when path exists", () => {
      const ts = new DocTabStore();
      const t1 = makeTab("doc1", "/my/file.pdf");
      ts.addTab(t1.doc, t1.store, t1.takeoffStore);
      const found = ts.findByPath("/my/file.pdf");
      expect(found?.docId).toBe("doc1");
    });

    it("returns undefined when path not open", () => {
      const ts = new DocTabStore();
      expect(ts.findByPath("/not/open.pdf")).toBeUndefined();
    });

    it("dedup: opening same path switches to existing tab (no duplicate added)", () => {
      const ts = new DocTabStore();
      const t1 = makeTab("doc1", "/shared.pdf");
      ts.addTab(t1.doc, t1.store, t1.takeoffStore);
      // App's dedup logic: findByPath -> switchTab (no addTab)
      const existing = ts.findByPath("/shared.pdf");
      if (existing) ts.switchTab(existing.docId);
      expect(ts.tabs.length).toBe(1);
      expect(ts.activeDocId).toBe("doc1");
    });
  });

  describe("closeTab", () => {
    it("closing the only tab results in empty state", () => {
      const ts = new DocTabStore();
      const t1 = makeTab("doc1");
      ts.addTab(t1.doc, t1.store, t1.takeoffStore);
      const next = ts.closeTab("doc1");
      expect(ts.tabs.length).toBe(0);
      expect(ts.activeDocId).toBeNull();
      expect(next).toBeNull();
    });

    it("closing the active tab activates the left neighbor", () => {
      const ts = new DocTabStore();
      const t1 = makeTab("doc1");
      const t2 = makeTab("doc2");
      const t3 = makeTab("doc3");
      ts.addTab(t1.doc, t1.store, t1.takeoffStore);
      ts.addTab(t2.doc, t2.store, t2.takeoffStore);
      ts.addTab(t3.doc, t3.store, t3.takeoffStore);
      ts.switchTab("doc2"); // active = doc2 (index 1)
      ts.closeTab("doc2");
      expect(ts.activeDocId).toBe("doc1"); // left neighbor
      expect(ts.tabs.length).toBe(2);
    });

    it("closing the first active tab (no left) activates right neighbor", () => {
      const ts = new DocTabStore();
      const t1 = makeTab("doc1");
      const t2 = makeTab("doc2");
      ts.addTab(t1.doc, t1.store, t1.takeoffStore);
      ts.addTab(t2.doc, t2.store, t2.takeoffStore);
      ts.switchTab("doc1"); // active = first tab
      ts.closeTab("doc1");
      expect(ts.activeDocId).toBe("doc2"); // right neighbor
      expect(ts.tabs.length).toBe(1);
    });

    it("closing the last active tab with one remaining activates remaining tab", () => {
      const ts = new DocTabStore();
      const t1 = makeTab("doc1");
      const t2 = makeTab("doc2");
      ts.addTab(t1.doc, t1.store, t1.takeoffStore);
      ts.addTab(t2.doc, t2.store, t2.takeoffStore);
      // doc2 is active (most recently added)
      ts.closeTab("doc2");
      expect(ts.activeDocId).toBe("doc1");
      expect(ts.tabs.length).toBe(1);
    });

    it("closing a background tab does not change the active tab", () => {
      const ts = new DocTabStore();
      const t1 = makeTab("doc1");
      const t2 = makeTab("doc2");
      const t3 = makeTab("doc3");
      ts.addTab(t1.doc, t1.store, t1.takeoffStore);
      ts.addTab(t2.doc, t2.store, t2.takeoffStore);
      ts.addTab(t3.doc, t3.store, t3.takeoffStore);
      ts.switchTab("doc2");
      ts.closeTab("doc3"); // background tab
      expect(ts.activeDocId).toBe("doc2");
      expect(ts.tabs.length).toBe(2);
    });

    it("closeTab returns the new activeDocId", () => {
      const ts = new DocTabStore();
      const t1 = makeTab("doc1");
      const t2 = makeTab("doc2");
      ts.addTab(t1.doc, t1.store, t1.takeoffStore);
      ts.addTab(t2.doc, t2.store, t2.takeoffStore);
      const next = ts.closeTab("doc2");
      expect(next).toBe("doc1");
    });
  });

  describe("saveViewportSnapshot", () => {
    it("updates the snapshot for the specified tab", () => {
      const ts = new DocTabStore();
      const t1 = makeTab("doc1");
      ts.addTab(t1.doc, t1.store, t1.takeoffStore);
      ts.saveViewportSnapshot("doc1", { zoom: 2.5, pageIndex: 3, scrollX: 100, scrollY: 200 });
      const tab = ts.tabs.find((t) => t.docId === "doc1");
      expect(tab?.viewportSnapshot).toEqual({
        zoom: 2.5,
        pageIndex: 3,
        scrollX: 100,
        scrollY: 200,
      });
    });

    it("does not throw for an unknown docId", () => {
      const ts = new DocTabStore();
      expect(() => {
        ts.saveViewportSnapshot("unknown", { zoom: 2, pageIndex: 0, scrollX: 0, scrollY: 0 });
      }).not.toThrow();
    });

    it("per-tab snapshots are independent", () => {
      const ts = new DocTabStore();
      const t1 = makeTab("doc1");
      const t2 = makeTab("doc2");
      ts.addTab(t1.doc, t1.store, t1.takeoffStore);
      ts.addTab(t2.doc, t2.store, t2.takeoffStore);
      ts.saveViewportSnapshot("doc1", { zoom: 2.0, pageIndex: 1, scrollX: 50, scrollY: 75 });
      ts.saveViewportSnapshot("doc2", { zoom: 0.5, pageIndex: 4, scrollX: 10, scrollY: 20 });
      expect(ts.tabs[0].viewportSnapshot.zoom).toBe(2.0);
      expect(ts.tabs[1].viewportSnapshot.zoom).toBe(0.5);
    });

    it("snapshot is preserved across tab switches", () => {
      const ts = new DocTabStore();
      const t1 = makeTab("doc1");
      const t2 = makeTab("doc2");
      ts.addTab(t1.doc, t1.store, t1.takeoffStore);
      ts.addTab(t2.doc, t2.store, t2.takeoffStore);
      ts.switchTab("doc1");
      ts.saveViewportSnapshot("doc1", { zoom: 3.0, pageIndex: 2, scrollX: 0, scrollY: 0 });
      ts.switchTab("doc2");
      ts.switchTab("doc1");
      const tab = ts.tabs.find((t) => t.docId === "doc1");
      expect(tab?.viewportSnapshot.zoom).toBe(3.0);
      expect(tab?.viewportSnapshot.pageIndex).toBe(2);
    });
  });

  describe("moveTab", () => {
    function makeThreeTabs() {
      const ts = new DocTabStore();
      const t1 = makeTab("doc1");
      const t2 = makeTab("doc2");
      const t3 = makeTab("doc3");
      ts.addTab(t1.doc, t1.store, t1.takeoffStore);
      ts.addTab(t2.doc, t2.store, t2.takeoffStore);
      ts.addTab(t3.doc, t3.store, t3.takeoffStore);
      return ts;
    }

    it("moves a tab forward (index 0 -> 2)", () => {
      const ts = makeThreeTabs();
      ts.moveTab(0, 2);
      expect(ts.tabs.map((t) => t.docId)).toEqual(["doc2", "doc3", "doc1"]);
    });

    it("moves a tab backward (index 2 -> 0)", () => {
      const ts = makeThreeTabs();
      ts.moveTab(2, 0);
      expect(ts.tabs.map((t) => t.docId)).toEqual(["doc3", "doc1", "doc2"]);
    });

    it("moves a tab to the first position (index 1 -> 0)", () => {
      const ts = makeThreeTabs();
      ts.moveTab(1, 0);
      expect(ts.tabs.map((t) => t.docId)).toEqual(["doc2", "doc1", "doc3"]);
    });

    it("moves a tab to the last position (index 1 -> 2)", () => {
      const ts = makeThreeTabs();
      ts.moveTab(1, 2);
      expect(ts.tabs.map((t) => t.docId)).toEqual(["doc1", "doc3", "doc2"]);
    });

    it("clamps toIndex above bounds to last index", () => {
      const ts = makeThreeTabs();
      ts.moveTab(0, 99);
      expect(ts.tabs.map((t) => t.docId)).toEqual(["doc2", "doc3", "doc1"]);
    });

    it("clamps toIndex below zero to 0", () => {
      const ts = makeThreeTabs();
      ts.moveTab(2, -5);
      expect(ts.tabs.map((t) => t.docId)).toEqual(["doc3", "doc1", "doc2"]);
    });

    it("same-position move is a no-op", () => {
      const ts = makeThreeTabs();
      const before = ts.tabs.map((t) => t.docId);
      ts.moveTab(1, 1);
      expect(ts.tabs.map((t) => t.docId)).toEqual(before);
    });

    it("single-tab store: moveTab is a no-op", () => {
      const ts = new DocTabStore();
      const t1 = makeTab("doc1");
      ts.addTab(t1.doc, t1.store, t1.takeoffStore);
      ts.moveTab(0, 0);
      expect(ts.tabs.map((t) => t.docId)).toEqual(["doc1"]);
    });

    it("active tab stays active after moving it", () => {
      const ts = makeThreeTabs();
      ts.switchTab("doc1"); // activate first tab then move it
      ts.moveTab(0, 2);
      expect(ts.activeDocId).toBe("doc1");
    });

    it("active tab stays active when a different tab is moved", () => {
      const ts = makeThreeTabs();
      ts.switchTab("doc2");
      ts.moveTab(0, 2); // move doc1, active is doc2
      expect(ts.activeDocId).toBe("doc2");
    });

    it("clamps fromIndex above bounds: no-op (out of range source)", () => {
      const ts = makeThreeTabs();
      const before = ts.tabs.map((t) => t.docId);
      ts.moveTab(99, 0);
      expect(ts.tabs.map((t) => t.docId)).toEqual(before);
    });

    it("clamps fromIndex below zero: no-op (out of range source)", () => {
      const ts = makeThreeTabs();
      const before = ts.tabs.map((t) => t.docId);
      ts.moveTab(-1, 2);
      expect(ts.tabs.map((t) => t.docId)).toEqual(before);
    });
  });
});
