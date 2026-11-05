import type { MarketTab, PublicConfig, TrainingItem, TrainingState, TrainingStep, TrainingTarget } from "../../../shared/types";

/* ------------------------------------------------------------------ */
/*  Auto Train helpers: sources, plan estimate, pipeline step mapping   */
/* ------------------------------------------------------------------ */

export type Source = MarketTab | "watchlist";

export const SOURCE_OPTIONS: ReadonlyArray<{ value: Source; label: string; title: string }> = [
	{ value: "live", label: "Live", title: "Tokens livestreaming now" },
	{ value: "new", label: "New", title: "Freshly created tokens" },
	{ value: "all", label: "All", title: "Top tokens by market cap" },
	{ value: "graduated", label: "Graduated", title: "Bonding curve complete" },
	{ value: "watchlist", label: "Watchlist", title: "Tokens you starred" },
];

/** Bun caps a run at this many tokens. */
export const MAX_TARGETS = 100;
export const MIN_INTERVAL = 6;
export const MAX_INTERVAL = 600;

/** A row in the setup checklist. */
export interface PickToken extends TrainingTarget {
	mcap?: number;
}

export const EMPTY_PICKS: PickToken[] = [];
export const EMPTY_MINTS: string[] = [];

/* ---- Engine ---------------------------------------------------------- */

export interface Engine {
	/** "Rules", "Claude claude-opus-5-5" */
	label: string;
	ai: boolean;
	/** AI chosen but its key is missing: the run falls back to rules. */
	missingKey: boolean;
	provider?: "Claude" | "OpenAI";
}

export function engineOf(cfg: PublicConfig | null): Engine {
	if (!cfg || cfg.orchestrator === "manual") return { label: "Rules", ai: false, missingKey: false };
	if (cfg.orchestrator === "claude")
		return { label: `Claude ${cfg.claudeModel}`, ai: true, missingKey: !cfg.claudeKeySet, provider: "Claude" };
	return { label: `OpenAI ${cfg.openaiModel}`, ai: true, missingKey: !cfg.openaiKeySet, provider: "OpenAI" };
}

/* ---- Time estimate ----------------------------------------------------- */

/** Rough seconds spent fetching, analyzing and submitting one token. */
export function perTokenSec(ai: boolean): number {
	return ai ? 8 : 2;
}

export function estimateSec(count: number, intervalSec: number, ai: boolean): number {
	if (count <= 0) return 0;
	return (count - 1) * Math.max(MIN_INTERVAL, intervalSec) + count * perTokenSec(ai);
}

/** "about 40 s", "about 3 min", "about 1 h 20 min" */
export function about(sec: number): string {
	if (sec < 60) return `about ${Math.max(1, Math.round(sec))} s`;
	const min = Math.round(sec / 60);
	if (min < 60) return `about ${min} min`;
	return `about ${Math.floor(min / 60)} h ${min % 60} min`;
}

/* ---- Run state --------------------------------------------------------- */

export const ACTIVE_STEPS: ReadonlySet<TrainingStep> = new Set(["fetching", "analyzing", "submitting"]);
export const isTerminal = (s: TrainingStep) => s === "done" || s === "error" || s === "skipped";

export function processedCount(t: TrainingState): number {
	let n = 0;
	for (const it of t.items) if (isTerminal(it.step)) n++;
	return n;
}

export function currentIndex(t: TrainingState): number {
	return t.items.findIndex((it) => ACTIVE_STEPS.has(it.step));
}

export type RunPhase = "none" | "running" | "stopping" | "finished";

export function phaseOf(t: TrainingState): RunPhase {
	if (t.status === "running") return "running";
	if (t.status === "stopping") return "stopping";
	return t.items.length > 0 ? "finished" : "none";
}

export function wasStopped(t: TrainingState): boolean {
	return t.items.some((it) => it.step === "skipped" && it.error === "Stopped");
}

export function retryTargets(t: TrainingState): TrainingTarget[] {
	return t.items
		.filter((it) => it.step === "error" || it.step === "skipped")
		.map(({ mint, symbol, name, image }) => ({ mint, symbol, name, image }));
}

/* ---- Pipeline chips ---------------------------------------------------- */

export type StageId = "fetch" | "analyze" | "submit";
export type StageState = "pending" | "current" | "done" | "failed";

export const STAGES: ReadonlyArray<{ id: StageId; label: string }> = [
	{ id: "fetch", label: "Fetch" },
	{ id: "analyze", label: "Analyze" },
	{ id: "submit", label: "Submit" },
];

const STEP_STAGE: Partial<Record<TrainingStep, number>> = { fetching: 0, analyzing: 1, submitting: 2 };

/**
 * State of the three stages for one item. For errors, `lastActive` is the
 * last in-flight step this view saw for the item; without it we infer from
 * the fields that were filled in.
 */
export function stagesFor(it: TrainingItem, lastActive?: TrainingStep): StageState[] {
	switch (it.step) {
		case "queued":
		case "skipped":
			return ["pending", "pending", "pending"];
		case "done":
			return ["done", "done", "done"];
		case "error": {
			const failedAt = lastActive != null ? (STEP_STAGE[lastActive] ?? 0) : it.sentiment ? 2 : 0;
			return [0, 1, 2].map((i) => (i < failedAt ? "done" : i === failedAt ? "failed" : "pending"));
		}
		default: {
			const at = STEP_STAGE[it.step] ?? 0;
			return [0, 1, 2].map((i) => (i < at ? "done" : i === at ? "current" : "pending"));
		}
	}
}

export const STEP_LABEL: Record<TrainingStep, string> = {
	queued: "Queued",
	fetching: "Fetching snapshot",
	analyzing: "Analyzing",
	submitting: "Submitting",
	done: "Submitted",
	skipped: "Skipped",
	error: "Failed",
};
