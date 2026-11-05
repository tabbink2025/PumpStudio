import { useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { AlertTriangle, ArrowDown, Check, ChevronRight, Cpu, Loader2, RotateCw, X } from "lucide-react";
import { shortMint, VALIDATION_DEVIATION_PCT } from "../../../shared/analysis";
import type { TrainingItem, TrainingState, TrainingStep } from "../../../shared/types";
import { cn } from "../../lib/cn";
import { clockTime, duration, integer, plural } from "../../lib/format";
import { useApp } from "../../lib/store";
import { SetupBanner } from "../shell/chrome";
import { Meter, SentimentBadge, TokenAvatar } from "../token/widgets";
import { Button } from "../ui/Button";
import { Badge, Banner } from "../ui/primitives";
import {
	about,
	ACTIVE_STEPS,
	currentIndex,
	isNotRun,
	isTerminal,
	notRunCount,
	perTokenSec,
	phaseOf,
	processedCount,
	retryTargets,
	STAGES,
	stagesFor,
	STEP_LABEL,
	upNextIndex,
	wasStopped,
	type RunPhase,
	type StageState,
} from "./model";

/* ---- Clock ------------------------------------------------------------ */

function useNow(enabled: boolean, ms = 1000): number {
	const [now, setNow] = useState(() => Date.now());
	useEffect(() => {
		if (!enabled) return;
		setNow(Date.now());
		const t = setInterval(() => setNow(Date.now()), ms);
		return () => clearInterval(t);
	}, [enabled, ms]);
	return now;
}

/* ---- Status pill ---------------------------------------------------------- */

export function StatusPill({ phase, stopped }: { phase: RunPhase; stopped?: boolean }) {
	const map: Record<RunPhase, { label: string; cls: string; dot: string; pulse?: boolean }> = {
		running: { label: "Running", cls: "border-brand/30 bg-brand/10 text-brand", dot: "bg-brand", pulse: true },
		stopping: { label: "Stopping", cls: "border-warn/30 bg-warn/10 text-warn", dot: "bg-warn", pulse: true },
		finished: { label: "Finished", cls: "border-line-strong bg-surface-3 text-fg-2", dot: "bg-fg-3" },
		none: { label: "Idle", cls: "border-line-strong bg-surface-3 text-fg-3", dot: "bg-fg-disabled" },
	};
	const m = phase === "finished" && stopped ? { ...map.finished, label: "Stopped" } : map[phase];
	return (
		<span className={cn("inline-flex h-6 shrink-0 items-center gap-1.5 rounded-full border px-2.5 text-sm font-medium", m.cls)}>
			<span aria-hidden className={cn("size-1.5 rounded-full", m.dot, m.pulse && "animate-pulse")} />
			{m.label}
		</span>
	);
}

/* ---- KPI tiles ------------------------------------------------------------- */

function Tile({ label, children, sub, title }: { label: string; children: ReactNode; sub?: ReactNode; title?: string }) {
	return (
		<div className="min-w-0 rounded-lg border border-line bg-surface-2 px-3 py-2.5" title={title}>
			<div className="truncate text-caption uppercase text-fg-3">{label}</div>
			<div className="num mt-0.5 truncate text-lg font-semibold text-fg">{children}</div>
			{sub && <div className="num mt-0.5 truncate text-sm text-fg-3">{sub}</div>}
		</div>
	);
}

function Kpis({ t, phase, now }: { t: TrainingState; phase: RunPhase; now: number }) {
	const processed = processedCount(t);
	const notRun = notRunCount(t);
	const pctDone = t.total > 0 ? (processed / t.total) * 100 : 0;
	const validatedPct = t.completed > 0 ? Math.round((t.validatedCount / t.completed) * 100) : null;
	const remaining = Math.max(0, t.total - processed - notRun);
	const skippedOther = Math.max(0, t.skipped - notRun);
	const failedSub = [`${integer(skippedOther)} skipped`];
	if (notRun > 0) failedSub.push(`${integer(notRun)} not run`);
	const ai = t.orchestrator !== "manual";

	let nextLabel = "Next token";
	let nextValue: ReactNode = "—";
	let nextSub: ReactNode = null;
	if (phase === "running" && t.nextAt) {
		const secs = Math.max(0, Math.ceil((t.nextAt - now) / 1000));
		nextValue = `in ${secs}s`;
		nextSub = remaining > 0 ? `${about(remaining * perTokenSec(ai) + (remaining - 1) * t.intervalSec + secs)} left` : null;
	} else if (phase === "running") {
		nextLabel = "Time left";
		nextValue = remaining > 0 ? about(remaining * perTokenSec(ai) + Math.max(0, remaining - 1) * t.intervalSec).replace("about ", "~") : "—";
		nextSub = "Working on a token";
	} else if (phase === "stopping") {
		nextLabel = "Next token";
		nextValue = "None";
		nextSub = "Finishing the current token";
	} else if (phase === "finished" && t.startedAt && t.finishedAt) {
		nextLabel = "Duration";
		nextValue = duration(t.finishedAt - t.startedAt);
		nextSub = `Ended ${clockTime(t.finishedAt)}`;
	}

	return (
		<div className="grid grid-cols-6 gap-2 xl:grid-cols-5 [&>*:nth-child(-n+3)]:col-span-2 [&>*:nth-child(n+4)]:col-span-3 xl:[&>*]:!col-span-1">
			<div className="min-w-0 rounded-lg border border-line bg-surface-2 px-3 py-2.5">
				<div className="truncate text-caption uppercase text-fg-3">Progress</div>
				<div className="num mt-0.5 truncate text-lg font-semibold text-fg">
					{integer(processed)} <span className="text-fg-3">/ {integer(t.total)}</span>
				</div>
				<Meter value={pctDone} className="mt-2" label="Run progress" tone={phase === "finished" ? "pos" : "brand"} />
			</div>
			<Tile label="XP earned" sub={t.completed > 0 ? `${plural(t.completed, "submission")}` : "No submissions yet"}>
				<span className={t.xpEarned > 0 ? "text-brand" : "text-fg"}>+{integer(t.xpEarned)}</span>
			</Tile>
			<Tile
				label="Validated"
				sub={validatedPct != null ? `${validatedPct}% within ${VALIDATION_DEVIATION_PCT}%` : `Snapshot within ${VALIDATION_DEVIATION_PCT}% of live data`}
				title={`A submission is validated when its snapshot is within ${VALIDATION_DEVIATION_PCT}% of the live data on pump.studio`}
			>
				{integer(t.validatedCount)} <span className="text-fg-3">of {integer(t.completed)}</span>
			</Tile>
			<Tile label="Failed" sub={failedSub.join(" · ")} title={notRun > 0 ? "Not run: still queued when the run stopped" : undefined}>
				<span className={cn("inline-flex items-center gap-1", t.failed > 0 ? "text-neg" : "text-fg")}>
					{t.failed > 0 && <X className="size-4" aria-hidden />}
					{integer(t.failed)}
				</span>
			</Tile>
			<Tile label={nextLabel} sub={nextSub}>
				{nextValue}
			</Tile>
		</div>
	);
}

/* ---- Pipeline chips ------------------------------------------------------- */

const STAGE_STYLE: Record<StageState, string> = {
	done: "border-pos/25 bg-pos/10 text-pos",
	current: "border-brand/30 bg-brand/10 text-brand",
	failed: "border-neg/30 bg-neg/10 text-neg",
	pending: "border-line bg-surface-2 text-fg-disabled",
};
const SHORT_STEP: Record<TrainingStep, string> = {
	queued: "Queued",
	fetching: "Fetching",
	analyzing: "Analyzing",
	submitting: "Submitting",
	done: "Done",
	skipped: "Skipped",
	error: "Failed",
};
const STAGE_WORD: Record<StageState, string> = { done: "done", current: "in progress", failed: "failed", pending: "pending" };

function StageIcon({ state }: { state: StageState }) {
	if (state === "done") return <Check className="size-3" strokeWidth={3} aria-hidden />;
	if (state === "failed") return <X className="size-3" strokeWidth={3} aria-hidden />;
	if (state === "current") return <span aria-hidden className="size-1.5 animate-pulse rounded-full bg-current" />;
	return <span aria-hidden className="size-1.5 rounded-full border border-current" />;
}

function Pipeline({ item, lastActive }: { item: TrainingItem; lastActive?: TrainingStep }) {
	if (item.step === "skipped")
		return isNotRun(item) ? (
			<span className="text-sm text-fg-3" title="Still queued when the run stopped">
				Not run
			</span>
		) : (
			<Badge tone="neutral" title={item.error}>
				Skipped
			</Badge>
		);
	const states = stagesFor(item, lastActive);
	return (
		<div className="flex min-w-0 items-center">
			<ol className="flex shrink-0 items-center" aria-label={`Pipeline: ${SHORT_STEP[item.step]}`}>
				{STAGES.map((s, i) => (
					<li key={s.id} className="flex items-center">
						{i > 0 && <ChevronRight className="mx-px size-3 text-fg-disabled" aria-hidden />}
						<span
							title={`${s.label}: ${STAGE_WORD[states[i]!]}`}
							className={cn(
								"inline-flex h-5 min-w-5 items-center justify-center gap-1 rounded border px-1 text-[11px] font-medium leading-none",
								STAGE_STYLE[states[i]!],
							)}
						>
							<StageIcon state={states[i]!} />
							<span className="sr-only 2xl:not-sr-only">{s.label}</span>
							<span className="sr-only 2xl:hidden">: {STAGE_WORD[states[i]!]}</span>
						</span>
					</li>
				))}
			</ol>
		</div>
	);
}

/* ---- Run table ------------------------------------------------------------- */

function noteFor(it: TrainingItem): { text: string; tone: "neg" | "warn" | "muted" } | null {
	if (!it.error || isNotRun(it)) return null;
	if (it.step === "error") return { text: it.error, tone: "neg" };
	if (it.step === "skipped") return { text: it.error, tone: "muted" };
	return { text: it.error, tone: "warn" };
}

const NOTE_CLS = { neg: "text-neg", warn: "text-warn", muted: "text-fg-3" };

function Engine({ it }: { it: TrainingItem }) {
	if (!it.source) return <span className="text-fg-disabled">—</span>;
	if (it.source === "heuristic") {
		const fellBack = it.error?.startsWith("AI fallback");
		return (
			<span className={cn("inline-flex items-center gap-1 text-sm", fellBack ? "text-warn" : "text-fg-2")} title={fellBack ? it.error : "Rule-based analysis"}>
				{fellBack && <AlertTriangle className="size-3" aria-hidden />}
				Rules
			</span>
		);
	}
	return (
		<Badge tone="violet" title={it.source === "claude" ? "Analyzed by Claude" : "Analyzed by OpenAI"}>
			AI
		</Badge>
	);
}

function Validated({ it }: { it: TrainingItem }) {
	if (it.step !== "done" || it.validated == null) return <span className="text-fg-disabled">—</span>;
	const dev = it.deviationPct != null ? `Snapshot ${it.deviationPct.toFixed(1)}% off live data` : undefined;
	if (it.validated)
		return (
			<span className="inline-flex items-center gap-1 text-pos" title={dev}>
				<Check className="size-3.5" strokeWidth={2.5} aria-hidden />
				Yes
			</span>
		);
	return (
		<span className="inline-flex items-center gap-1 text-warn" title={dev ? `${dev} (limit ${VALIDATION_DEVIATION_PCT}%)` : "Not validated"}>
			<AlertTriangle className="size-3.5" aria-hidden />
			No
		</span>
	);
}

function RunTable({ t, now, follow, onFollow }: { t: TrainingState; now: number; follow: boolean; onFollow: (v: boolean) => void }) {
	const openAnalyze = useApp((s) => s.openAnalyze);
	const scroller = useRef<HTMLDivElement>(null);
	const programmatic = useRef(0);
	const current = currentIndex(t);
	const upNext = upNextIndex(t);
	// The row the table follows: the token in flight, or the next one during the wait.
	const focus = current >= 0 ? current : upNext;

	// Fallback for states without failedStep: the last in-flight step this view saw.
	const lastActive = useRef<{ runId: number; map: Map<string, TrainingStep> }>({ runId: -1, map: new Map() });
	if (lastActive.current.runId !== t.runId) lastActive.current = { runId: t.runId, map: new Map() };
	for (const it of t.items) if (ACTIVE_STEPS.has(it.step)) lastActive.current.map.set(it.mint, it.step);

	useLayoutEffect(() => {
		if (!follow || focus < 0) return;
		const box = scroller.current;
		const el = box?.querySelector<HTMLElement>(`[data-row="${focus}"]`);
		if (!el || !box) return;
		const top = el.offsetTop - box.clientHeight / 2 + el.offsetHeight / 2;
		programmatic.current = Date.now();
		box.scrollTo({ top: Math.max(0, top), behavior: "smooth" });
	}, [focus, follow]);

	function onScroll() {
		if (Date.now() - programmatic.current < 700) return;
		const box = scroller.current;
		if (!box || focus < 0) return;
		const el = box.querySelector<HTMLElement>(`[data-row="${focus}"]`);
		if (!el) return;
		const visible = el.offsetTop >= box.scrollTop + 32 && el.offsetTop + el.offsetHeight <= box.scrollTop + box.clientHeight;
		if (visible !== follow) onFollow(visible);
	}

	const nextSecs = t.nextAt ? Math.max(0, Math.ceil((t.nextAt - now) / 1000)) : null;

	return (
		<div className="relative flex min-h-0 flex-1 flex-col overflow-hidden rounded-lg border border-line bg-surface-2">
			<div ref={scroller} onScroll={onScroll} className="min-h-0 flex-1 overflow-y-auto">
				<table className="w-full table-fixed border-collapse text-base">
					<caption className="sr-only">Tokens in this training run</caption>
					<thead className="sticky top-0 z-10 bg-surface-2">
						<tr className="h-9 border-b border-line text-left text-caption uppercase text-fg-3">
							<th scope="col" className="hidden w-[80px] pl-4 font-medium xl:table-cell">
								Time
							</th>
							<th scope="col" className="pl-4 font-medium xl:pl-3 2xl:w-[168px]">
								Token
							</th>
							<th scope="col" className="w-[108px] px-3 font-medium 2xl:w-[228px]" title="Fetch snapshot, analyze, submit">
								Pipeline
							</th>
							<th scope="col" className="w-[112px] px-3 font-medium">
								Result
							</th>
							<th scope="col" className="hidden w-[68px] px-3 font-medium xl:table-cell">
								Engine
							</th>
							<th scope="col" className="w-[56px] px-3 text-right font-medium">
								XP
							</th>
							<th scope="col" className="w-[96px] px-3 font-medium">
								Validated
							</th>
							<th scope="col" className="hidden pr-4 font-medium 2xl:table-cell">
								Notes
							</th>
						</tr>
					</thead>
					<tbody>
						{t.items.map((it, i) => {
							const isCurrent = i === current;
							const isNext = i === upNext;
							const note = noteFor(it);
							const symbol = it.symbol ? `$${it.symbol}` : shortMint(it.mint);
							// Rows without an outcome yet (queued, in flight, not run) leave the result cells empty.
							const open = !isTerminal(it.step) || isNotRun(it);
							const dim = it.step === "queued" || isNotRun(it);
							// Left edge marker for the row in flight (brand) or up next (muted); sits in the first visible cell.
							const marker =
								isCurrent || isNext ? (
									<span aria-hidden className={cn("absolute inset-y-1.5 left-0 w-0.5 rounded-full", isCurrent ? "bg-brand" : "bg-line-control")} />
								) : null;
							return (
								<tr
									key={it.mint}
									data-row={i}
									aria-current={isCurrent ? "step" : undefined}
									className={cn(
										"h-row border-b border-line last:border-b-0",
										isCurrent ? "bg-brand/5" : "hover:bg-surface-3/50",
										dim && "text-fg-3",
									)}
								>
									<td className="num relative hidden whitespace-nowrap pl-4 text-sm text-fg-3 xl:table-cell">
										{marker}
										{isNext ? <span className="text-fg-2">Next</span> : it.step === "queued" ? "Queued" : clockTime(it.updatedAt)}
									</td>
									<td className="relative min-w-0 pl-4 xl:pl-3">
										<span className="xl:hidden">{marker}</span>
										<button
											type="button"
											onClick={() => openAnalyze(it.mint)}
											title={`Open ${symbol}${it.name ? ` (${it.name})` : ""} in Analyze`}
											className="group flex max-w-full items-center gap-2 rounded text-left"
										>
											<TokenAvatar mint={it.mint} src={it.image} symbol={it.symbol} size={20} />
											<span className="min-w-0">
												<span className={cn("block truncate font-medium group-hover:underline", dim ? "text-fg-2" : "text-fg")}>{symbol}</span>
												{note && (
													<span className={cn("block truncate text-sm 2xl:hidden", NOTE_CLS[note.tone])} title={note.text}>
														{note.text}
													</span>
												)}
											</span>
										</button>
									</td>
									<td className="px-3">
										<Pipeline item={it} lastActive={lastActive.current.map.get(it.mint)} />
									</td>
									<td className="px-3">
										{it.sentiment ? (
											<SentimentBadge value={it.sentiment} score={it.score} />
										) : isNext && nextSecs != null ? (
											<span className="num whitespace-nowrap text-sm text-fg-2">Starts in {nextSecs}s</span>
										) : open ? null : (
											<span className="text-fg-disabled">—</span>
										)}
									</td>
									<td className="hidden px-3 xl:table-cell">{open && !it.source ? null : <Engine it={it} />}</td>
									<td className="num px-3 text-right">
										{it.step === "done" && it.xpEarned != null ? (
											<span className={cn("font-semibold", it.xpEarned > 0 ? "text-brand" : "text-fg-3")}>+{integer(it.xpEarned)}</span>
										) : open ? null : (
											<span className="text-fg-disabled">—</span>
										)}
									</td>
									<td className="px-3">{open ? null : <Validated it={it} />}</td>
									<td className="hidden pr-4 2xl:table-cell">
										{note ? (
											<span className={cn("block truncate text-sm", NOTE_CLS[note.tone])} title={note.text}>
												{note.text}
											</span>
										) : null}
									</td>
								</tr>
							);
						})}
					</tbody>
				</table>
			</div>
			{!follow && focus >= 0 && (
				<div className="pointer-events-none absolute inset-x-0 bottom-3 flex justify-center">
					<Button size="sm" className="pointer-events-auto shadow-pop" icon={<ArrowDown className="size-3.5" />} onClick={() => onFollow(true)}>
						Jump to current token
					</Button>
				</div>
			)}
		</div>
	);
}

/* ---- Empty + summary ------------------------------------------------------- */

const HOW = [
	{ n: 1, title: "Pick tokens", text: "Choose a source on the left and tick the tokens you want labeled." },
	{ n: 2, title: "Your agent does the work", text: "For each token it fetches a fresh snapshot, analyzes it and submits the result." },
	{ n: 3, title: "Earn XP", text: "Validated submissions earn XP: the snapshot must match live data within 15%, so each token is fetched fresh." },
];

function NoRunYet() {
	return (
		<div className="flex min-h-0 flex-1 items-center justify-center overflow-y-auto rounded-lg border border-dashed border-line-strong bg-surface-2/50">
			<div className="w-full max-w-md px-6 py-10">
				{/* EmptyState's own padding cannot be overridden (cn does not merge), so this header is local. */}
				<div className="flex flex-col items-center text-center">
					<div className="mb-4 flex size-11 items-center justify-center rounded-xl border border-line-strong bg-surface-2 text-fg-3" aria-hidden>
						<Cpu className="size-5" />
					</div>
					<h2 className="text-md font-semibold text-fg">No training run yet</h2>
					<p className="mt-1 max-w-sm text-base text-fg-3">Auto Train submits analyses for a batch of tokens while you do something else.</p>
				</div>
				<ol className="mt-6 space-y-3 rounded-lg border border-line bg-surface-2 p-4">
					{HOW.map((h) => (
						<li key={h.n} className="flex items-start gap-3">
							<span className="num flex size-6 shrink-0 items-center justify-center rounded-full border border-line-strong bg-surface-3 text-sm font-semibold text-fg-2">
								{h.n}
							</span>
							<div className="min-w-0">
								<div className="text-base font-medium text-fg">{h.title}</div>
								<div className="text-sm text-fg-3">{h.text}</div>
							</div>
						</li>
					))}
				</ol>
			</div>
		</div>
	);
}

function Summary({ t, onRetry, onClear, canRetry }: { t: TrainingState; onRetry: () => void; onClear: () => void; canRetry: boolean }) {
	const retry = retryTargets(t).length;
	const stopped = wasStopped(t);
	const notRun = notRunCount(t);
	const retryLabel =
		notRun === retry ? `Run remaining (${integer(retry)})` : notRun === 0 && retry === t.failed ? `Retry failed (${integer(retry)})` : `Retry ${plural(retry, "token")}`;
	const parts = [`${integer(t.completed)} submitted`, `+${integer(t.xpEarned)} XP`];
	if (t.failed > 0) parts.push(`${integer(t.failed)} failed`);
	const skippedOther = t.skipped - notRun;
	if (skippedOther > 0) parts.push(`${integer(skippedOther)} skipped`);
	if (notRun > 0) parts.push(`${integer(notRun)} not run`);
	const tone = t.completed === 0 && t.failed > 0 ? "neg" : t.failed > 0 || stopped ? "warn" : "pos";
	return (
		<Banner
			tone={tone}
			title={`${stopped ? "Stopped" : "Finished"}: ${parts.join(", ")}`}
			action={
				<div className="flex items-center gap-1.5">
					{retry > 0 && (
						<Button
							size="sm"
							icon={<RotateCw className="size-3.5" />}
							onClick={onRetry}
							disabled={!canRetry}
							title={canRetry ? `Start a new run with the ${plural(retry, "token")} that failed or did not run` : "Add your pump.studio API key first"}
						>
							{retryLabel}
						</Button>
					)}
					<Button size="sm" variant="ghost" onClick={onClear}>
						Clear
					</Button>
				</div>
			}
		>
			{t.lastError && t.failed > 0 ? <span className="block truncate" title={t.lastError}>Last error: {t.lastError}</span> : null}
		</Banner>
	);
}

/* ---- Panel ----------------------------------------------------------------- */

export function RunPanel({
	t: state,
	onRetry,
	canRetry,
	engineLabel,
	liveIntervalSec,
}: {
	t: TrainingState;
	onRetry: () => void;
	canRetry: boolean;
	engineLabel: string;
	/** Bun reads the interval from config before every wait, so a change applies to the running batch. */
	liveIntervalSec?: number;
}) {
	const phase = phaseOf(state);
	const t = useMemo(
		() => (phase === "running" && liveIntervalSec != null && liveIntervalSec !== state.intervalSec ? { ...state, intervalSec: liveIntervalSec } : state),
		[state, phase, liveIntervalSec],
	);
	const now = useNow(phase === "running" && !!t.nextAt);
	const [follow, setFollow] = useState(true);
	const [clearedRun, setClearedRun] = useState(-1);
	const current = currentIndex(t);
	const currentItem = current >= 0 ? t.items[current] : undefined;

	// Follow the new run from the top.
	useEffect(() => setFollow(true), [t.runId]);

	const meta = useMemo(() => {
		const bits: string[] = [];
		if (t.startedAt) bits.push(`Started ${clockTime(t.startedAt)}`);
		bits.push(`every ${t.intervalSec} s`);
		bits.push(engineLabel);
		return bits.join(" · ");
	}, [t.startedAt, t.intervalSec, engineLabel]);

	return (
		<div className="flex min-h-0 min-w-0 flex-1 flex-col gap-3 overflow-hidden p-5">
			<SetupBanner />
			{phase === "none" ? (
				<NoRunYet />
			) : (
				<>
					<div className="flex min-w-0 items-center gap-3">
						<StatusPill phase={phase} stopped={phase === "finished" && wasStopped(t)} />
						<span className="min-w-0 flex-1 truncate text-sm text-fg-3" title={meta}>
							{phase === "running" && currentItem ? (
								<span className="text-fg-2">
									<Loader2 className="mr-1 inline size-3.5 animate-spin align-[-2px] text-brand" aria-hidden />
									{STEP_LABEL[currentItem.step]} {currentItem.symbol ? `$${currentItem.symbol}` : shortMint(currentItem.mint)}
									<span className="text-fg-3"> · </span>
								</span>
							) : null}
							{meta}
						</span>
					</div>
					{phase === "finished" && clearedRun !== t.runId && (
						<Summary t={t} onRetry={onRetry} canRetry={canRetry} onClear={() => setClearedRun(t.runId)} />
					)}
					<Kpis t={t} phase={phase} now={now} />
					<RunTable t={t} now={now} follow={follow} onFollow={setFollow} />
				</>
			)}
		</div>
	);
}
