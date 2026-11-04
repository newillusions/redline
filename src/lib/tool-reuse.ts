/**
 * Single-use tools (Revu's "Reuse Tools" off): which tool a freshly made markup finishes.
 * Pure; the MarkupStore applies it when reuse is off.
 */

/** Tools whose whole point is a run of items: the Count tool keeps counting. */
const RUN_TOOLS = new Set(["MeasurementCount"]);

/**
 * True when creating a markup of `markupType` completes a use of `activeTool`, so with reuse
 * off the tool should drop back to Select. The draw tools are named after the markup they
 * make; Cloud+ makes a Cloud (and its callout), and a Tool Chest Drawing-mode tool places
 * whatever its template holds.
 */
export function finishesTool(activeTool: string, markupType: string): boolean {
  if (RUN_TOOLS.has(activeTool)) return false;
  if (activeTool === "placeTool") return true;
  if (activeTool === "CloudPlus") return markupType === "Cloud";
  return activeTool === markupType;
}
