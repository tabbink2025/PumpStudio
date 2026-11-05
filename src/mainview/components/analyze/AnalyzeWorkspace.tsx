import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
	AlertCircle,
	ArrowLeftRight,
	CheckCircle2,
	KeyRound,
	Loader2,
	RotateCcw,
	RotateCw,
	Send,
	Settings2,
	Sparkles,
	XCircle,
} from "lucide-react";
import {
	SNAPSHOT_MAX_AGE_MS,
	SUMMARY_MAX,
	buildPayload,
	computeDefaults,
	sanitizeFields,
	validateFields,
} from "../../../shared/analysis";
import type { AnalysisFields, DataPoint, SubmitResult } from "../../../shared/types";
import { cn } from "../../lib/cn";
import { duration } from "../../lib/format";
import { isModKey, overlayOpen } from "../../lib/hotkeys";
import { bun, errorMessage } from "../../lib/rpc";
import { toast, useApp } from "../../lib/store";
import { Button } from "../ui/Button";
import { Banner, Card, EmptyState, MOD, Skeleton } from "../ui/primitives";
import { AnalysisForm, FIELD_SECTION, sectionId, summaryId } from "./AnalysisForm";
import { getDraft, saveDraft, snapshotTime, type FieldKey, type FillSource } from "./drafts";
import { useNow } from "./hooks";
import { ResultPanel, useNextToken } from "./ResultPanel";
import { TokenPanel, TokenPanelSkeleton } from "./TokenPanel";

const ALL_KEYS: FieldKey[] = [
	"sentiment",
	"score",
	"summary",
	"riskLevel",
	"riskFactors",
	"buyPressure",
	"volatilityScore",
	"liquidityDepth",
	"holderConcentration",
	"trendDirection",
	"volumeProfile",
];

const engineName = (e: string) => (e === "openai" ? "OpenAI" : "Claude");

/** One token's analysis: data on the left, form on the right. Keyed by mint. */
export function AnalyzeWorkspace({ mint, active }: { mint: string; active: boolean }) {
	const draft0 = useMemo(() => getDraft(mint), [mint]);
	const [dp, setDp] = useState<DataPoint | null>(draft0?.dp ?? null);
	const [fetchedAt, setFetchedAt] = useState(draft0?.fetchedAt ?? 0);
	const [loading, setLoading] = useState(false);
	const [loadError, setLoadError] = useState<string | null>(null);
	const [fields, setFields] = useState<AnalysisFields | null>(draft0?.fields ?? null);
	const [aiFields, setAiFields] = useState<FieldKey[]>(draft0?.aiFields ?? []);
	const [source, setSource] = useState<FillSource>(draft0?.source ?? { kind: "rules" });
	const [result, setResult] = useState<SubmitResult | null>(draft0?.result ?? null);
	const [submitting, setSubmitting] = useState(false);
	const [submitError, setSubmitError] = useState<string | null>(null);
	const [aiStartedAt, setAiStartedAt] = useState<number | null>(null);
	const [triedSubmit, setTriedSubmit] = useState(false);

	const apiKeySet = useApp((s) => !!s.config?.apiKeySet);
	const orchestrator = useApp((s) => s.config?.orchestrator ?? "manual");
	const claudeKeySet = useApp((s) => !!s.config?.claudeKeySet);
	const openaiKeySet = useApp((s) => !!s.config?.openaiKeySet);
	const refreshTick = useApp((s) => s.refreshTick);
	const setView = useApp((s) => s.setView);
	const openAnalyze = useApp((s) => s.openAnalyze);
	const rememberTokens = useApp((s) => s.rememberTokens);
	const recordAnalysis = useApp((s) => s.recordAnalysis);
	const marketLoading = useApp((s) => !!s.market.all?.loading);
	const next = useNextToken(mint);

	const mounted = useRef(true);
	useEffect(() => {
		mounted.current = true;
		return () => {
			mounted.current = false;
		};
	}, []);

	const now = useNow(aiStartedAt ? 1000 : 5000, active);
	const snapAt = snapshotTime(dp, fetchedAt);
	const defaults = useMemo(() => (dp ? computeDefaults(dp) : null), [dp]);
	const errors = useMemo(() => (fields ? validateFields(fields) : {}), [fields]);
	const errorKeys = Object.keys(errors) as FieldKey[];

	/* ---- Persist the draft ------------------------------------------------ */

	const latest = useRef({ fields, aiFields, source, result, dp, fetchedAt });
	latest.current = { fields, aiFields, source, result, dp, fetchedAt };
	useEffect(() => {
		if (fields) saveDraft(mint, { fields, aiFields, source, result, dp, fetchedAt });
	}, [mint, fields, aiFields, source, result, dp, fetchedAt]);

	/* ---- Data -------------------------------------------------------------- */

	const loadReq = useRef(0);
	const load = useCallback(
		async (opts?: { silent?: boolean }): Promise<DataPoint | null> => {
			const id = ++loadReq.current;
			setLoading(true);
			setLoadError(null);
			try {
				const d = await bun.getDataPoint(mint);
				if (id !== loadReq.current || !mounted.current) return null;
				setDp(d);
				setFetchedAt(Date.now());
				setFields((f) => f ?? computeDefaults(d));
				rememberTokens([{ mint, symbol: d.symbol, name: d.name, image: d.imageUrl }]);
				return d;
			} catch (err) {
				if (id !== loadReq.current || !mounted.current) return null;
				const msg = errorMessage(err);
				setLoadError(msg);
				if (latest.current.dp && !opts?.silent) {
					toast({ kind: "error", title: "Could not refresh the snapshot", description: msg });
				}
				return null;
			} finally {
				if (id === loadReq.current && mounted.current) setLoading(false);
			}
		},
		[mint, rememberTokens],
	);

	// First load: reuse a fresh cached snapshot, otherwise fetch.
	useEffect(() => {
		const d = getDraft(mint);
		if (!d?.dp || Date.now() - snapshotTime(d.dp, d.fetchedAt) > SNAPSHOT_MAX_AGE_MS) void load();
	}, [mint, load]);

	// Global refresh (Cmd+R / menu) while this screen is visible.
	const lastTick = useRef(refreshTick);
	useEffect(() => {
		if (refreshTick === lastTick.current) return;
		lastTick.current = refreshTick;
		if (active) void load();
	}, [refreshTick, active, load]);

	/* ---- Editing ---------------------------------------------------------- */

	const setField = useCallback(<K extends FieldKey>(key: K, value: AnalysisFields[K]) => {
		setFields((f) => (f ? { ...f, [key]: value } : f));
		setAiFields((a) => (a.includes(key) ? a.filter((k) => k !== key) : a));
		setSubmitError(null);
	}, []);

	function resetToData() {
		if (!dp || !fields) return;
		const prev = { fields, aiFields, source };
		setFields(computeDefaults(dp));
		setAiFields([]);
		setSource({ kind: "rules" });
		toast({
			kind: "info",
			title: "Form reset to the data values",
			action: {
				label: "Undo",
				run: () => {
					if (!mounted.current) return;
					setFields(prev.fields);
					setAiFields(prev.aiFields);
					setSource(prev.source);
				},
			},
			duration: 5000,
		});
	}

	function insertSummary() {
		if (!defaults || !fields) return;
		const cur = fields.summary.trim();
		const text = cur ? `${cur}\n\n${defaults.summary}` : defaults.summary;
		setField("summary", text.slice(0, SUMMARY_MAX));
		requestAnimationFrame(() => {
			const el = document.getElementById(summaryId(mint)) as HTMLTextAreaElement | null;
			el?.focus();
			el?.setSelectionRange(el.value.length, el.value.length);
		});
	}

	/* ---- AI auto-fill ------------------------------------------------------ */

	const providerKeySet = orchestrator === "claude" ? claudeKeySet : orchestrator === "openai" ? openaiKeySet : false;
	const aiReady = orchestrator !== "manual" && providerKeySet;
	const provider = orchestrator === "manual" ? "AI" : engineName(orchestrator);
	const aiBlockedReason =
		orchestrator === "manual"
			? "AI auto-fill is off. Choose Claude or OpenAI in Settings."
			: !providerKeySet
				? `Add your ${provider} API key in Settings to auto-fill.`
				: undefined;

	const aiReq = useRef(0);
	async function autofill() {
		if (!dp || !fields || aiStartedAt || !aiReady || result) return;
		const id = ++aiReq.current;
		const started = Date.now();
		setAiStartedAt(started);
		try {
			const r = await bun.orchestrateAnalysis({ dataPoint: dp });
			if (id !== aiReq.current || !mounted.current || useApp.getState().analyzeMint !== mint) return;
			const { source: src, model, fallbackReason, durationMs, ...raw } = r;
			const nextFields = sanitizeFields(raw, computeDefaults(dp));
			const prev = { fields: latest.current.fields, aiFields: latest.current.aiFields, source: latest.current.source };
			setFields(nextFields);
			setSubmitError(null);
			if (fallbackReason) {
				setSource({ kind: "fallback", reason: fallbackReason, durationMs });
				setAiFields([]);
			} else if (src === "heuristic" || src === "manual") {
				setSource({ kind: "rules" });
				setAiFields([]);
			} else {
				setSource({ kind: "ai", engine: src, model, durationMs });
				setAiFields(ALL_KEYS);
				toast({
					kind: "success",
					title: `Filled by ${engineName(src)}`,
					description: "Review the values before you submit.",
					action: {
						label: "Undo",
						run: () => {
							if (!mounted.current || !prev.fields) return;
							setFields(prev.fields);
							setAiFields(prev.aiFields);
							setSource(prev.source);
						},
					},
					duration: 5000,
				});
			}
		} catch (err) {
			if (id !== aiReq.current || !mounted.current) return;
			toast({ kind: "error", title: `${provider} auto-fill failed`, description: errorMessage(err) });
		} finally {
			if (id === aiReq.current && mounted.current) setAiStartedAt(null);
		}
	}

	function cancelAi() {
		aiReq.current++;
		setAiStartedAt(null);
	}

	/* ---- Submit ------------------------------------------------------------ */

	const submittingRef = useRef(false);
	function jumpToFirstError() {
		const key = errorKeys[0];
		if (!key) return;
		if (key === "summary") {
			const el = document.getElementById(summaryId(mint));
			el?.scrollIntoView({ block: "center", behavior: "smooth" });
			el?.focus({ preventScroll: true });
			return;
		}
		const el = document.getElementById(sectionId(mint, FIELD_SECTION[key]));
		el?.scrollIntoView({ block: "start", behavior: "smooth" });
		el?.focus({ preventScroll: true });
	}

	async function submit() {
		if (submittingRef.current || !fields || !dp || result) return;
		setTriedSubmit(true);
		if (!apiKeySet) return;
		if (errorKeys.length > 0) {
			jumpToFirstError();
			return;
		}
		submittingRef.current = true;
		setSubmitting(true);
		setSubmitError(null);
		const sent = fields;
		try {
			let d = dp;
			if (Date.now() - snapshotTime(dp, fetchedAt) > SNAPSHOT_MAX_AGE_MS) {
				const fresh = await load({ silent: true });
				if (!fresh) throw new Error("Could not refresh the snapshot before submitting. Check your connection and try again.");
				d = fresh;
			}
			const res = await bun.submitAnalysis(buildPayload(d, sent));
			if (!res.ok) throw new Error(res.error || "The server did not accept this analysis");
			const xp = res.xpEarned ?? 0;
			recordAnalysis(mint, { sentiment: sent.sentiment, score: sent.score, xp, validated: res.validated });
			toast({
				kind: "xp",
				title: xp > 0 ? `+${xp} XP` : "Analysis submitted",
				description: `$${d.symbol} · ${res.validated ? "Validated" : "Not validated"}`,
			});
			if (mounted.current) setResult(res);
			else {
				const dr = getDraft(mint);
				if (dr) saveDraft(mint, { ...dr, result: res });
			}
		} catch (err) {
			const msg = errorMessage(err);
			if (mounted.current) setSubmitError(msg);
			else toast({ kind: "error", title: "Submit failed", description: msg });
		} finally {
			submittingRef.current = false;
			if (mounted.current) setSubmitting(false);
		}
	}

	function analyzeAgain() {
		setResult(null);
		setTriedSubmit(false);
		setSubmitError(null);
		void load();
	}

	/* ---- Keyboard (works while typing, only on this screen) ---------------- */

	const handlers = useRef({ submit, autofill, next: () => {} });
	handlers.current = {
		submit,
		autofill,
		next: () => {
			if (next) openAnalyze(next.mint);
		},
	};
	useEffect(() => {
		if (!active) return;
		const onKey = (e: KeyboardEvent) => {
			if (e.defaultPrevented || !isModKey(e) || e.altKey || e.shiftKey || overlayOpen()) return;
			if (e.key === "Enter") {
				e.preventDefault();
				if (latest.current.result) handlers.current.next();
				else void handlers.current.submit();
			} else if (e.key.toLowerCase() === "i") {
				e.preventDefault();
				void handlers.current.autofill();
			}
		};
		window.addEventListener("keydown", onKey);
		return () => window.removeEventListener("keydown", onKey);
	}, [active]);

	/* ---- Render ------------------------------------------------------------ */

	if (!dp && loadError) {
		return (
			<div className="flex min-h-0 flex-1 items-center justify-center">
				<EmptyState
					icon={<XCircle className="size-5 text-neg" />}
					title={/not found|404/i.test(loadError) ? "Token not found" : "Could not load this token"}
					description={loadError}
					action={
						<>
							<Button onClick={() => void load()} icon={<RotateCw className="size-4" />}>
								Try again
							</Button>
							<Button variant="ghost" onClick={() => openAnalyze(null)} icon={<ArrowLeftRight className="size-4" />}>
								Pick another token
							</Button>
						</>
					}
				/>
			</div>
		);
	}

	if (!dp || !fields || !defaults) return <WorkspaceSkeleton />;

	const aiRunning = aiStartedAt != null;

	return (
		<div className="flex min-h-0 flex-1">
			<aside
				aria-label="Token data"
				className="relative w-[312px] shrink-0 overflow-y-auto border-r border-line bg-surface-1 xl:w-[348px]"
			>
				<TokenPanel
					dp={dp}
					snapshotAt={snapAt}
					now={now}
					refreshing={loading}
					onRefresh={() => void load()}
					onChangeToken={() => openAnalyze(null)}
				/>
			</aside>

			<div className="flex min-w-0 flex-1 flex-col">
				{result ? (
					<div className="relative min-h-0 flex-1 overflow-y-auto">
						<ResultPanel
							symbol={dp.symbol}
							result={result}
							fields={fields}
							againLoading={loading}
							next={next}
							marketLoading={marketLoading}
							onNext={(m) => openAnalyze(m)}
							onAgain={analyzeAgain}
							onBack={() => setView("tokens")}
						/>
					</div>
				) : (
					<>
						<div className="relative min-h-0 flex-1 overflow-y-auto">
							<div className="mx-auto w-full max-w-[780px] px-6 pb-8 pt-5">
								{/* Form top bar */}
								<div className="mb-4 flex flex-wrap items-center gap-x-3 gap-y-2">
									<div className="flex min-w-[240px] flex-1 items-center gap-2.5">
										<h2 className="shrink-0 text-md font-semibold text-fg">Your analysis</h2>
										{aiRunning ? (
											<span className="inline-flex min-w-0 items-center gap-1.5 text-sm text-violet" role="status">
												<Loader2 className="size-3.5 shrink-0 animate-spin" aria-hidden />
												<span className="truncate">
													Asking {provider} about ${dp.symbol}
												</span>
												<span className="num text-fg-3">{duration(now - aiStartedAt)}</span>
											</span>
										) : (
											<SourceChip source={source} />
										)}
									</div>
									<div className="ml-auto flex shrink-0 items-center gap-2">
										{aiRunning ? (
											<Button variant="ghost" size="sm" onClick={cancelAi} title="Keep the current values">
												Cancel
											</Button>
										) : (
											<Button
												variant="ghost"
												size="sm"
												icon={<RotateCcw className="size-3.5" />}
												onClick={resetToData}
												title="Recompute every field from the snapshot"
											>
												Reset to data
											</Button>
										)}
										{!aiReady && (
											<Button
												variant="ghost"
												size="sm"
												icon={<Settings2 className="size-3.5" />}
												onClick={() => setView("settings")}
												title={aiBlockedReason}
											>
												Set up AI
											</Button>
										)}
										<span title={aiBlockedReason} className="inline-flex">
											<Button
												variant="ai"
												icon={<Sparkles className="size-4" />}
												loading={aiRunning}
												disabled={!aiReady}
												onClick={() => void autofill()}
												kbd={`${MOD} I`}
												aria-describedby={undefined}
											>
												{aiRunning ? `Asking ${provider}…` : `Auto-fill with ${provider}`}
											</Button>
										</span>
									</div>
								</div>

								{source.kind === "fallback" && !aiRunning && (
									<Banner tone="warn" className="mb-4">
										{provider} failed: {source.reason.replace(/\.$/, "")}. Values come from the rule-based analysis.
									</Banner>
								)}

								<div
									aria-busy={aiRunning || undefined}
									className={cn(
										"transition-opacity duration-base",
										aiRunning && "pointer-events-none select-none opacity-50",
									)}
								>
									<AnalysisForm
										mint={mint}
										fields={fields}
										defaults={defaults}
										aiFields={aiFields}
										errors={errors}
										showErrors={triedSubmit}
										onChange={setField}
										onInsertSummary={insertSummary}
									/>
								</div>
							</div>
						</div>

						{/* Sticky footer */}
						<div className="shrink-0 border-t border-line bg-surface-1">
							<div className="mx-auto w-full max-w-[780px] px-6 py-3">
								{submitError && (
									<Banner tone="neg" title="Submit failed" className="mb-3">
										{submitError}
									</Banner>
								)}
								<div className="flex items-center gap-3">
									<div className="min-w-0 flex-1">
										{!apiKeySet ? (
											<div className="flex min-w-0 items-center gap-2 text-base">
												<KeyRound className="size-4 shrink-0 text-brand" aria-hidden />
												<span className="truncate text-fg-2">
													<span className="font-medium text-fg">Connect your pump.studio key</span> to submit and earn XP.
												</span>
												<Button size="sm" variant="secondary" onClick={() => setView("settings")}>
													Set up
												</Button>
											</div>
										) : errorKeys.length > 0 ? (
											<button
												type="button"
												onClick={jumpToFirstError}
												className="flex min-w-0 max-w-full items-center gap-2 rounded text-left text-base text-neg hover:underline"
												title={errorKeys.map((k) => errors[k]).join("\n")}
											>
												<AlertCircle className="size-4 shrink-0" aria-hidden />
												<span className="truncate">
													Fix {errorKeys.length === 1 ? "1 issue" : `${errorKeys.length} issues`}: {errors[errorKeys[0]!]}
												</span>
											</button>
										) : (
											<div className="flex items-center gap-2 text-base text-pos" role="status">
												<CheckCircle2 className="size-4 shrink-0" aria-hidden />
												<span>Ready to submit</span>
											</div>
										)}
									</div>
									<Button
										variant="primary"
										size="lg"
										icon={<Send className="size-4" />}
										loading={submitting}
										disabled={!apiKeySet || aiRunning}
										onClick={() => void submit()}
										kbd={`${MOD} ↵`}
									>
										{submitting ? "Submitting…" : "Submit analysis"}
									</Button>
								</div>
							</div>
						</div>
					</>
				)}
			</div>
		</div>
	);
}

function Chip({ tone, icon, title, children }: { tone: "neutral" | "violet" | "warn"; icon?: ReactNode; title: string; children: ReactNode }) {
	const tones = {
		neutral: "bg-surface-3 text-fg-2 border-line-strong",
		violet: "bg-violet/10 text-violet border-violet/25",
		warn: "bg-warn/10 text-warn border-warn/25",
	};
	return (
		<span
			title={title}
			className={cn(
				"inline-flex h-5 min-w-0 items-center gap-1 rounded border px-1.5 text-[11px] font-medium leading-none",
				tones[tone],
			)}
		>
			{icon}
			<span className="truncate">{children}</span>
		</span>
	);
}

function SourceChip({ source }: { source: FillSource }) {
	if (source.kind === "ai") {
		const secs = `${(source.durationMs / 1000).toFixed(1)}s`;
		const text = [`Filled by ${engineName(source.engine)}`, source.model, secs].filter(Boolean).join(" · ");
		return (
			<Chip tone="violet" icon={<Sparkles className="size-3 shrink-0" aria-hidden />} title={text}>
				{text}
			</Chip>
		);
	}
	if (source.kind === "fallback") {
		return (
			<Chip tone="warn" title={source.reason}>
				Rule-based fallback
			</Chip>
		);
	}
	return (
		<Chip tone="neutral" title="Every field was computed from the snapshot with the reference rules">
			Pre-filled from data rules
		</Chip>
	);
}

function WorkspaceSkeleton() {
	return (
		<div className="flex min-h-0 flex-1" aria-busy aria-label="Loading token">
			<aside className="w-[312px] shrink-0 border-r border-line bg-surface-1 xl:w-[348px]">
				<TokenPanelSkeleton />
			</aside>
			<div className="relative min-w-0 flex-1 overflow-hidden">
				<div className="mx-auto w-full max-w-[780px] space-y-4 px-6 pt-5">
					<div className="flex items-center justify-between">
						<Skeleton className="h-5 w-48" />
						<Skeleton className="h-8 w-44" />
					</div>
					{[120, 220, 260].map((h, i) => (
						<Card key={i} className="p-5">
							<Skeleton className="h-3 w-24" />
							<Skeleton className="mt-2 h-3 w-64" />
							<div className="mt-5 grid grid-cols-2 gap-6">
								<Skeleton className="h-8" />
								<Skeleton className="h-8" />
							</div>
							<div style={{ height: h - 100 }} />
						</Card>
					))}
				</div>
			</div>
		</div>
	);
}
