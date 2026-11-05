import { buildPayload } from "../shared/analysis";
import type { AgentConfig, TrainingItem, TrainingState, TrainingTarget, XpState } from "../shared/types";
import { ApiError, type PumpStudioApi } from "./api";
import type { Orchestrator } from "./orchestrator";

/* ------------------------------------------------------------------ */
/*  Auto-training loop + session XP                                     */
/*                                                                      */
/*  State lives here (not in the view), so switching screens or        */
/*  reloading the UI never loses a running session. Every change is    */
/*  pushed to the view as a full snapshot.                              */
/* ------------------------------------------------------------------ */

/** Server-side cooldown when the same mint is resubmitted. */
const RESUBMIT_COOLDOWN_MS = 120_000;
const MAX_TARGETS = 100;

export class Trainer {
	private state: TrainingState = {
		status: "idle",
		runId: 0,
		total: 0,
		completed: 0,
		failed: 0,
		skipped: 0,
		xpEarned: 0,
		validatedCount: 0,
		intervalSec: 10,
		orchestrator: "manual",
		items: [],
	};
	private xp: XpState = { session: 0, submissions: 0, validated: 0 };
	private lastSubmitted = new Map<string, number>();
	private wake: (() => void) | null = null;

	constructor(
		private api: PumpStudioApi,
		private orchestrator: Orchestrator,
		private getConfig: () => AgentConfig,
		private onState: (s: TrainingState) => void,
		private onXp: (x: XpState) => void,
	) {}

	getState(): TrainingState {
		return structuredClone(this.state);
	}

	getXp(): XpState {
		return { ...this.xp };
	}

	/** Record a submission made from the Analyze screen or the loop. */
	recordSubmit(mint: string, result: { xpEarned?: number; xpTotal?: number; validated?: boolean }): void {
		this.lastSubmitted.set(mint, Date.now());
		this.xp.submissions++;
		if (result.validated) this.xp.validated++;
		if (typeof result.xpEarned === "number" && Number.isFinite(result.xpEarned)) this.xp.session += result.xpEarned;
		if (typeof result.xpTotal === "number" && Number.isFinite(result.xpTotal)) this.xp.total = result.xpTotal;
		this.onXp(this.getXp());
	}

	start(targets: TrainingTarget[]): TrainingState {
		if (this.state.status !== "idle") throw new Error("A training run is already active");
		const cfg = this.getConfig();
		if (!cfg.apiKey) throw new Error("Add your pump.studio API key in Settings first");
		const unique = [...new Map(targets.filter((t) => t?.mint).map((t) => [t.mint, t])).values()].slice(0, MAX_TARGETS);
		if (unique.length === 0) throw new Error("Select at least one token");

		const now = Date.now();
		this.state = {
			status: "running",
			runId: this.state.runId + 1,
			startedAt: now,
			total: unique.length,
			completed: 0,
			failed: 0,
			skipped: 0,
			xpEarned: 0,
			validatedCount: 0,
			intervalSec: cfg.analyzeInterval,
			orchestrator: cfg.orchestrator,
			items: unique.map((t) => ({ ...t, step: "queued", updatedAt: now })),
		};
		this.emit();
		void this.run(this.state.runId);
		return this.getState();
	}

	stop(): TrainingState {
		if (this.state.status === "running") {
			this.state.status = "stopping";
			this.wake?.(); // interrupt the wait between tokens
			this.emit();
		}
		return this.getState();
	}

	private emit(): void {
		this.onState(this.getState());
	}

	private update(index: number, patch: Partial<TrainingItem>): void {
		const item = this.state.items[index];
		if (!item) return;
		this.state.items[index] = { ...item, ...patch, updatedAt: Date.now() };
		this.emit();
	}

	private active(runId: number): boolean {
		return this.state.runId === runId && this.state.status === "running";
	}

	private sleep(ms: number): Promise<void> {
		return new Promise((resolve) => {
			const done = () => {
				clearTimeout(timer);
				this.wake = null;
				resolve();
			};
			const timer = setTimeout(done, ms);
			this.wake = done;
		});
	}

	private async run(runId: number): Promise<void> {
		const items = this.state.items;
		for (let i = 0; i < items.length; i++) {
			if (!this.active(runId)) break;
			const mint = items[i]!.mint;
			const cfg = this.getConfig();

			const last = this.lastSubmitted.get(mint);
			if (cfg.skipRecentlyAnalyzed && last && Date.now() - last < RESUBMIT_COOLDOWN_MS) {
				this.state.skipped++;
				this.update(i, { step: "skipped", error: "Analyzed less than 2 minutes ago" });
				continue;
			}

			try {
				this.update(i, { step: "fetching" });
				const dp = await this.api.getDataPoint(mint);
				this.update(i, {
					symbol: items[i]!.symbol ?? dp.symbol,
					name: items[i]!.name ?? dp.name,
					image: items[i]!.image ?? dp.imageUrl,
				});
				if (!this.active(runId)) break;

				this.update(i, { step: "analyzing" });
				const analysis = await this.orchestrator.analyze(dp);
				if (!this.active(runId)) break;

				this.update(i, { step: "submitting", sentiment: analysis.sentiment, score: analysis.score, source: analysis.source });
				const result = await this.api.submitAnalysis(buildPayload(dp, analysis));
				this.recordSubmit(mint, result);

				this.state.completed++;
				this.state.xpEarned += result.xpEarned ?? 0;
				if (result.validated) this.state.validatedCount++;
				this.update(i, {
					step: "done",
					xpEarned: result.xpEarned,
					validated: result.validated,
					deviationPct: result.deviationPct,
					error: analysis.fallbackReason ? `AI fallback: ${analysis.fallbackReason}` : result.warning,
				});
			} catch (err) {
				const message = err instanceof Error ? err.message : String(err);
				if (/cooldown/i.test(message)) {
					this.state.skipped++;
					this.update(i, { step: "skipped", error: "Token is on cooldown" });
				} else {
					const at = this.state.items[i]?.step;
					this.state.failed++;
					this.state.lastError = message;
					this.update(i, {
						step: "error",
						error: message,
						failedStep: at === "fetching" || at === "analyzing" || at === "submitting" ? at : undefined,
					});
				}
				if (err instanceof ApiError && (err.status === 401 || err.status === 403)) {
					this.state.status = "stopping"; // a bad key fails every token: stop early
					break;
				}
			}

			const hasMore = i < items.length - 1;
			if (hasMore && this.active(runId)) {
				const rateLimited = this.state.items[i]?.error?.toLowerCase().includes("rate limit");
				const waitMs = Math.max(this.getConfig().analyzeInterval, 6) * 1000 * (rateLimited ? 3 : 1);
				this.state.nextAt = Date.now() + waitMs;
				this.emit();
				await this.sleep(waitMs);
				this.state.nextAt = undefined;
			}
		}

		if (this.state.runId !== runId) return;
		const stopped = this.state.status === "stopping";
		this.state.items = this.state.items.map((it) =>
			it.step === "queued" ? { ...it, step: "skipped", error: stopped ? "Stopped" : it.error, updatedAt: Date.now() } : it,
		);
		this.state.skipped = this.state.items.filter((it) => it.step === "skipped").length;
		this.state.status = "idle";
		this.state.finishedAt = Date.now();
		this.state.nextAt = undefined;
		this.emit();
	}
}
