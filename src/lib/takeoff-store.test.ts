// @vitest-environment jsdom
import { describe, it, expect } from "vitest";
import { TakeoffStore } from "./takeoff-store.svelte";
import type { ScaleRecord } from "./ipc";

function makeScale(id: string, ratio = 0.001): ScaleRecord {
  return {
    id,
    applies_to: { kind: "DocumentDefault" },
    method: "TwoPoint",
    ratio,
    unit: "m",
    label: "1:1000",
    precision: 2,
  };
}

describe("TakeoffStore", () => {
  it("starts with empty scales and no active scale", () => {
    const s = new TakeoffStore();
    expect(s.scales).toHaveLength(0);
    expect(s.activeScaleId).toBeNull();
  });

  it("seedScales populates the list and activates the first", () => {
    const s = new TakeoffStore();
    s.seedScales([makeScale("s1"), makeScale("s2")]);
    expect(s.scales).toHaveLength(2);
    expect(s.activeScaleId).toBe("s1");
  });

  it("addScale inserts and sets as active", () => {
    const s = new TakeoffStore();
    s.addScale(makeScale("s1"));
    expect(s.scales).toHaveLength(1);
    expect(s.activeScaleId).toBe("s1");
  });

  it("addScale replaces a scale with the same id", () => {
    const s = new TakeoffStore();
    s.addScale(makeScale("s1", 0.001));
    s.addScale(makeScale("s1", 0.002));
    expect(s.scales).toHaveLength(1);
    expect(s.activeScale?.ratio).toBeCloseTo(0.002);
  });

  it("deleteScale removes the record", () => {
    const s = new TakeoffStore();
    s.addScale(makeScale("s1"));
    s.deleteScale("s1");
    expect(s.scales).toHaveLength(0);
    expect(s.activeScaleId).toBeNull();
  });

  it("activeScale returns the record matching activeScaleId", () => {
    const s = new TakeoffStore();
    s.addScale(makeScale("s1"));
    s.addScale(makeScale("s2", 0.002));
    s.activeScaleId = "s2";
    expect(s.activeScale?.ratio).toBeCloseTo(0.002);
  });

  it("calibrationState starts as null", () => {
    const s = new TakeoffStore();
    expect(s.calibrationState).toBeNull();
  });

  it("startCalibration sets state to waiting_p1", () => {
    const s = new TakeoffStore();
    s.startCalibration({ page: 0, appliesToPage: null });
    expect(s.calibrationState?.step).toBe("waiting_p1");
  });

  it("calibrationClickP1 advances to waiting_p2", () => {
    const s = new TakeoffStore();
    s.startCalibration({ page: 0, appliesToPage: null });
    s.calibrationClickP1({ x: 0, y: 0 });
    expect(s.calibrationState?.step).toBe("waiting_p2");
  });

  it("cancelCalibration clears state", () => {
    const s = new TakeoffStore();
    s.startCalibration({ page: 0, appliesToPage: null });
    s.cancelCalibration();
    expect(s.calibrationState).toBeNull();
  });
});
