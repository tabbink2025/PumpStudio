/* ------------------------------------------------------------------ */
/*  Unsent work per mint. Module-level on purpose: switching tokens or  */
/*  screens remounts the workspace, but the draft survives until the    */
/*  app restarts.                                                       */
/* ------------------------------------------------------------------ */

import type { AnalysisFields, DataPoint, Orchestrator, SubmitResult } from "../../../shared/types";

export type FieldKey = keyof AnalysisFields;

/** Where the current form values came from. */
export type FillSource =
	| { kind: "rules" }
	| { kind: "ai"; engine: Exclude<Orchestrator, "manual">; model?: string; durationMs: number }
	| { kind: "fallback"; reason: string; durationMs: number };

export interface Draft {
	fields: AnalysisFields;
	/** Fields last set by the AI and not edited since. */
	aiFields: FieldKey[];
	source: FillSource;
	/** Set after a successful submit; the result panel replaces the form. */
	result: SubmitResult | null;
	dp: DataPoint | null;
	/** When this view received the snapshot (fallback for dp.snapshotAt). */
	fetchedAt: number;
}

const drafts = new Map<string, Draft>();

export function getDraft(mint: string): Draft | undefined {
	return drafts.get(mint);
}

export function saveDraft(mint: string, d: Draft) {
	drafts.set(mint, d);
	// Keep memory bounded: drop the oldest drafts beyond 30.
	if (drafts.size > 30) {
		const first = drafts.keys().next().value;
		if (first) drafts.delete(first);
	}
}

export function snapshotTime(dp: DataPoint | null, fetchedAt: number): number {
	return typeof dp?.snapshotAt === "number" && dp.snapshotAt > 0 ? dp.snapshotAt : fetchedAt;
}
