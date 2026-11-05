import { memo, useEffect, useId, useState, type ReactNode } from "react";
import { AlertTriangle, Bot, Check, ChevronRight, EyeOff, RotateCw, Search, SearchX, Star, X } from "lucide-react";
import { shortMint } from "../../../shared/analysis";
import type { PublicConfig } from "../../../shared/types";
import { cn } from "../../lib/cn";
import { ago, integer, plural, titleCase, usdCompact } from "../../lib/format";
import { useApp, type MyAnalysis } from "../../lib/store";
import { TokenAvatar } from "../token/widgets";
import { IconButton } from "../ui/Button";
import { Badge, EmptyState, ErrorState, FieldLabel, FieldMessage, Segmented, Skeleton, Switch } from "../ui/primitives";
import {
	about,
	engineOf,
	estimateSec,
	MAX_INTERVAL,
	MAX_TARGETS,
	MIN_INTERVAL,
	SOURCE_OPTIONS,
	type PickToken,
	type Source,
} from "./model";

/* ---- Token checklist row --------------------------------------------- */

const TokenRow = memo(function TokenRow({
	token,
	checked,
	mine,
	onToggle,
}: {
	token: PickToken;
	checked: boolean;
	mine?: MyAnalysis;
	onToggle: (t: PickToken) => void;
}) {
	const symbol = token.symbol ? `$${token.symbol}` : shortMint(token.mint);
	return (
		<label
			title={token.name ? `${symbol} · ${token.name}` : token.mint}
			className={cn(
				"group relative flex h-9 items-center gap-2.5 rounded-md px-2 transition-colors duration-fast",
				checked ? "bg-brand/5 hover:bg-brand/10" : "hover:bg-surface-3",
			)}
		>
			<input type="checkbox" className="peer sr-only" checked={checked} onChange={() => onToggle(token)} />
			<span
				aria-hidden
				className={cn(
					"flex size-4 shrink-0 items-center justify-center rounded border transition-colors duration-fast",
					"peer-focus-visible:outline peer-focus-visible:outline-2 peer-focus-visible:outline-offset-2 peer-focus-visible:outline-focus",
					checked ? "border-brand bg-brand text-brand-fg" : "border-line-control bg-surface-2 group-hover:border-fg-3",
				)}
			>
				{checked && <Check className="size-3" strokeWidth={3} />}
			</span>
			<TokenAvatar mint={token.mint} src={token.image} symbol={token.symbol} size={20} />
			<span className="min-w-0 flex-1 truncate text-base">
				<span className="font-medium text-fg">{symbol}</span>
				{token.name && <span className="ml-1.5 text-fg-3">{token.name}</span>}
			</span>
			{mine && (
				<Badge
					tone="neutral"
					icon={<Check className="size-3" aria-hidden />}
					title={`You analyzed this ${ago(mine.at)}: ${titleCase(mine.sentiment)} ${mine.score}`}
				>
					Analyzed
				</Badge>
			)}
			<span className="num w-[52px] shrink-0 text-right text-sm text-fg-2">{token.mcap != null ? usdCompact(token.mcap) : ""}</span>
		</label>
	);
});

function ListSkeleton() {
	return (
		<div role="status" className="space-y-px px-2" aria-busy="true" aria-label="Loading tokens">
			{Array.from({ length: 9 }, (_, i) => (
				<div key={i} className="flex h-9 items-center gap-2.5 px-2">
					<Skeleton className="size-4 rounded" />
					<Skeleton className="size-5 rounded-full" />
					<Skeleton className="h-3 flex-1" />
					<Skeleton className="h-3 w-10" />
				</div>
			))}
		</div>
	);
}

/* ---- Small local controls ---------------------------------------------- */

function LinkButton({ children, onClick, disabled }: { children: ReactNode; onClick: () => void; disabled?: boolean }) {
	return (
		<button
			type="button"
			onClick={onClick}
			disabled={disabled}
			className="rounded text-sm font-medium text-fg-2 hover:text-fg hover:underline disabled:pointer-events-none disabled:text-fg-disabled"
		>
			{children}
		</button>
	);
}

function ToggleChip({ pressed, onClick, children, title }: { pressed: boolean; onClick: () => void; children: ReactNode; title?: string }) {
	return (
		<button
			type="button"
			aria-pressed={pressed}
			title={title}
			onClick={onClick}
			className={cn(
				"inline-flex h-8 shrink-0 items-center gap-1.5 rounded-md border px-2.5 text-sm font-medium transition-colors duration-fast",
				pressed ? "border-brand/30 bg-brand/10 text-brand" : "border-line-strong bg-surface-2 text-fg-2 hover:border-line-control hover:text-fg",
			)}
		>
			<span
				aria-hidden
				className={cn("flex size-3.5 items-center justify-center rounded-sm border", pressed ? "border-brand bg-brand text-brand-fg" : "border-line-control")}
			>
				{pressed && <Check className="size-2.5" strokeWidth={3} />}
			</span>
			{children}
		</button>
	);
}

/* ---- Settings block ------------------------------------------------------ */

function IntervalField({ config }: { config: PublicConfig | null }) {
	const id = useId();
	const hintId = useId();
	const saveConfig = useApp((s) => s.saveConfig);
	const saved = config?.analyzeInterval ?? 10;
	const [draft, setDraft] = useState(String(saved));
	const [flash, setFlash] = useState(false);
	useEffect(() => setDraft(String(saved)), [saved]);
	useEffect(() => {
		if (!flash) return;
		const t = setTimeout(() => setFlash(false), 1800);
		return () => clearTimeout(t);
	}, [flash]);

	const n = Number(draft);
	const invalid = draft.trim() === "" || !Number.isFinite(n) || n < MIN_INTERVAL || n > MAX_INTERVAL;

	async function commit() {
		const next = Math.round(Math.max(MIN_INTERVAL, Math.min(MAX_INTERVAL, Number.isFinite(n) && draft.trim() !== "" ? n : saved)));
		setDraft(String(next));
		if (next === saved) return;
		const res = await saveConfig({ analyzeInterval: next }, { silent: true });
		if (res) setFlash(true);
		else setDraft(String(saved));
	}

	return (
		<div>
			<FieldLabel htmlFor={id} hint={flash ? <span className="text-pos">Saved</span> : undefined}>
				Interval between tokens
			</FieldLabel>
			<div className="field flex h-8 w-32 items-center pr-2.5">
				<input
					id={id}
					type="number"
					inputMode="numeric"
					min={MIN_INTERVAL}
					max={MAX_INTERVAL}
					step={1}
					value={draft}
					disabled={!config}
					aria-invalid={invalid || undefined}
					aria-describedby={hintId}
					onChange={(e) => setDraft(e.target.value)}
					onBlur={() => void commit()}
					onKeyDown={(e) => {
						if (e.key === "Enter") (e.target as HTMLInputElement).blur();
					}}
					className="num h-full min-w-0 flex-1 bg-transparent px-2.5 text-base text-fg outline-none"
				/>
				<span className="text-sm text-fg-3" aria-hidden>
					sec
				</span>
			</div>
			<FieldMessage id={hintId} error={invalid && draft.trim() !== ""}>
				{invalid && draft.trim() !== "" && n < MIN_INTERVAL
					? `Minimum is ${MIN_INTERVAL} s. It is raised to ${MIN_INTERVAL} s when you leave the field.`
					: "Minimum 6 s. pump.studio accepts 10 submissions per minute."}
			</FieldMessage>
		</div>
	);
}

function EngineLine({ config }: { config: PublicConfig | null }) {
	const setView = useApp((s) => s.setView);
	const engine = engineOf(config);
	return (
		<div>
			<div className="text-sm font-medium text-fg-2">Analysis engine</div>
			<div className="mt-1.5 flex min-w-0 items-center gap-2 text-base text-fg" title={engine.label}>
				<Bot className={cn("size-4 shrink-0", engine.ai ? "text-violet" : "text-fg-3")} aria-hidden />
				<span className="min-w-0 truncate">{engine.label}</span>
				{engine.ai && <Badge tone="violet">AI</Badge>}
				<span className="flex-1" />
				<button
					type="button"
					onClick={() => setView("settings")}
					aria-label="Change analysis engine in Settings"
					className="shrink-0 rounded text-sm font-medium text-info hover:underline"
				>
					Change
				</button>
			</div>
			{engine.missingKey && (
				<p className="mt-1.5 flex items-start gap-1.5 text-sm text-warn">
					<AlertTriangle className="mt-0.5 size-3.5 shrink-0" aria-hidden />
					<span>No {engine.provider} API key. Every token falls back to rule-based analysis.</span>
				</p>
			)}
		</div>
	);
}

/* ---- Plan summary --------------------------------------------------------- */

export function PlanCard({ count, config, blocked, running }: { count: number; config: PublicConfig | null; blocked?: string; running: boolean }) {
	const engine = engineOf(config);
	const interval = config?.analyzeInterval ?? 10;
	const runCount = Math.min(count, MAX_TARGETS);
	const ai = engine.ai && !engine.missingKey;
	return (
		<div className="rounded-lg border border-line bg-surface-2 p-3">
			<div className="text-caption uppercase text-fg-3">Plan</div>
			{count === 0 ? (
				<p className="mt-1 text-base text-fg-3">Select tokens above to see the plan.</p>
			) : (
				<>
					<p className="num mt-1 text-base font-medium text-fg">
						{plural(runCount, "token")} · {about(estimateSec(runCount, interval, ai))} at {interval} s
					</p>
					<p className="mt-0.5 truncate text-sm text-fg-2" title={engine.label}>
						{ai ? `${engine.label} (up to ${plural(runCount, "AI call")})` : `${engine.label}, no AI calls`}
					</p>
					{count > MAX_TARGETS && (
						<p className="mt-1.5 text-sm text-warn">
							Only the first {MAX_TARGETS} of {integer(count)} run in one batch.
						</p>
					)}
				</>
			)}
			{running ? (
				<p className="mt-2 border-t border-line pt-2 text-sm text-fg-3">A run is in progress. You can prepare the next batch here.</p>
			) : (
				blocked &&
				count > 0 && (
					<p className="mt-2 flex items-start gap-1.5 border-t border-line pt-2 text-sm text-warn">
						<AlertTriangle className="mt-0.5 size-3.5 shrink-0" aria-hidden />
						{blocked}
					</p>
				)
			)}
		</div>
	);
}

const SETTINGS_OPEN_KEY = "pumpstudio.auto.settingsOpen";

function loadSettingsOpen(): boolean {
	try {
		return localStorage.getItem(SETTINGS_OPEN_KEY) !== "0";
	} catch {
		return true;
	}
}

/* ---- Panel --------------------------------------------------------------- */

export interface SetupPanelProps {
	source: Source;
	onSource: (s: Source) => void;
	query: string;
	onQuery: (q: string) => void;
	hideAnalyzed: boolean;
	onHideAnalyzed: (v: boolean) => void;
	/** Tokens in the current source after search + filters. */
	visible: PickToken[];
	/** Tokens in the current source before filters (for empty-state copy). */
	sourceCount: number;
	loading: boolean;
	error?: string;
	onRetry: () => void;
	selected: ReadonlyMap<string, PickToken>;
	hiddenSelected: number;
	onToggle: (t: PickToken) => void;
	onSelectAll: () => void;
	onSelectNone: () => void;
	onRemoveHidden: () => void;
	config: PublicConfig | null;
	blocked?: string;
	running: boolean;
}

export function SetupPanel(p: SetupPanelProps) {
	const searchId = useId();
	const settingsId = useId();
	const [settingsOpen, setSettingsOpen] = useState(loadSettingsOpen);
	const toggleSettings = (v: boolean) => {
		setSettingsOpen(v);
		try {
			localStorage.setItem(SETTINGS_OPEN_KEY, v ? "1" : "0");
		} catch {}
	};
	const settingsSummary = p.config
		? `Every ${p.config.analyzeInterval} s · ${p.config.skipRecentlyAnalyzed ? "skip recent" : "no skip"} · ${engineOf(p.config).label}`
		: "";
	const analyzed = useApp((s) => s.analyzed);
	const saveConfig = useApp((s) => s.saveConfig);
	const setView = useApp((s) => s.setView);
	const visibleSelected = p.selected.size - p.hiddenSelected;
	const allVisibleSelected = p.visible.length > 0 && p.visible.every((t) => p.selected.has(t.mint));

	let list: ReactNode;
	if (p.loading && p.sourceCount === 0) list = <ListSkeleton />;
	else if (p.error && p.sourceCount === 0) list = <ErrorState title="Could not load tokens" message={p.error} onRetry={p.onRetry} />;
	else if (p.sourceCount === 0 && p.source === "watchlist")
		list = (
			<EmptyState
				icon={<Star className="size-5" />}
				title="Your watchlist is empty"
				description="Star tokens on the Tokens screen to train on them here."
				action={
					<button type="button" onClick={() => setView("tokens")} className="rounded text-base font-medium text-info hover:underline">
						Go to Tokens
					</button>
				}
			/>
		);
	else if (p.sourceCount === 0)
		list = <EmptyState icon={<SearchX className="size-5" />} title="No tokens in this list" description="Try another source or refresh." />;
	else if (p.visible.length === 0)
		list = (
			<EmptyState
				icon={<SearchX className="size-5" />}
				title="No tokens match"
				description={p.hideAnalyzed ? "You already analyzed every token here, or none match your search." : "Nothing matches your search."}
				action={
					<button
						type="button"
						onClick={() => {
							p.onQuery("");
							p.onHideAnalyzed(false);
						}}
						className="rounded text-base font-medium text-info hover:underline"
					>
						Clear filters
					</button>
				}
			/>
		);
	else
		list = (
			<div role="group" aria-label="Tokens to train on" className="space-y-px px-2 pb-2">
				{p.visible.map((t) => (
					<TokenRow key={t.mint} token={t} checked={p.selected.has(t.mint)} mine={analyzed[t.mint]} onToggle={p.onToggle} />
				))}
			</div>
		);

	return (
		<aside aria-label="Training setup" className="flex w-[320px] shrink-0 flex-col overflow-y-auto border-r border-line bg-surface-1 xl:w-[340px] 2xl:w-[380px]">
			<div className="flex min-h-full flex-col">
				{/* Token picker */}
				<div className="space-y-2.5 px-4 pb-2 pt-4">
					<div className="flex items-center justify-between gap-2">
						<h2 className="text-caption uppercase text-fg-3">Tokens</h2>
						<IconButton label="Refresh list" size="sm" onClick={p.onRetry} disabled={p.loading || p.source === "watchlist"}>
							<RotateCw className={cn("size-3.5", p.loading && "animate-spin")} />
						</IconButton>
					</div>
					<Segmented label="Token source" size="sm" full value={p.source} options={SOURCE_OPTIONS} onChange={p.onSource} />
					<div className="flex items-center gap-2">
						<div className="field flex h-8 min-w-0 flex-1 items-center gap-1.5 px-2">
							<Search className="size-3.5 shrink-0 text-fg-3" aria-hidden />
							<label htmlFor={searchId} className="sr-only">
								Filter tokens
							</label>
							<input
								id={searchId}
								value={p.query}
								onChange={(e) => p.onQuery(e.target.value)}
								onKeyDown={(e) => {
									if (e.key === "Escape" && p.query) {
										e.stopPropagation();
										p.onQuery("");
									}
								}}
								placeholder="Filter by name or symbol"
								className="h-full min-w-0 flex-1 bg-transparent text-base text-fg outline-none placeholder:text-fg-3"
							/>
							{p.query && (
								<button
									type="button"
									aria-label="Clear filter"
									onClick={() => p.onQuery("")}
									className="rounded p-0.5 text-fg-3 hover:text-fg"
								>
									<X className="size-3.5" />
								</button>
							)}
						</div>
						<ToggleChip
							pressed={p.hideAnalyzed}
							onClick={() => p.onHideAnalyzed(!p.hideAnalyzed)}
							title="Hide tokens I already analyzed"
						>
							Hide analyzed
						</ToggleChip>
					</div>
					<div className="flex h-6 items-center gap-3 text-sm">
						<span className="num min-w-0 flex-1 truncate text-fg-2" aria-live="polite">
							<span className="font-medium text-fg">{integer(p.selected.size)}</span> selected
						</span>
						<LinkButton onClick={p.onSelectAll} disabled={p.visible.length === 0 || allVisibleSelected}>
							Select all
						</LinkButton>
						<LinkButton onClick={p.onSelectNone} disabled={visibleSelected === 0}>
							None
						</LinkButton>
					</div>
					{p.hiddenSelected > 0 && (
						<div className="flex items-center gap-2 rounded-md border border-info/25 bg-info/5 px-2.5 py-1.5 text-sm">
							<EyeOff className="size-3.5 shrink-0 text-info" aria-hidden />
							<span className="num min-w-0 flex-1 text-fg-2">
								{p.hiddenSelected === 1 ? "1 selected token is" : `${integer(p.hiddenSelected)} selected tokens are`} not in this list
							</span>
							<button type="button" onClick={p.onRemoveHidden} className="shrink-0 rounded font-medium text-info hover:underline">
								Remove
							</button>
						</div>
					)}
				</div>
				{p.error && p.sourceCount > 0 && (
					<p role="alert" className="mx-4 mb-2 flex items-center gap-1.5 text-sm text-neg">
						<AlertTriangle className="size-3.5 shrink-0" aria-hidden />
						<span className="min-w-0 flex-1 truncate" title={p.error}>
							Refresh failed: {p.error}
						</span>
						<button type="button" onClick={p.onRetry} className="shrink-0 rounded font-medium text-info hover:underline">
							Retry
						</button>
					</p>
				)}
				<div className="min-h-[216px] flex-1 basis-0 overflow-y-auto border-y border-line bg-bg/40 py-1 [&>div:not([role])]:py-6">{list}</div>

				{/* Settings */}
				<div className="px-4 py-3">
					<button
						type="button"
						aria-expanded={settingsOpen}
						aria-controls={settingsId}
						onClick={() => toggleSettings(!settingsOpen)}
						className="-mx-1 flex w-[calc(100%+8px)] items-center gap-2 rounded px-1 py-1 text-left"
					>
						<ChevronRight
							className={cn("size-3.5 shrink-0 text-fg-3 transition-transform duration-fast", settingsOpen && "rotate-90")}
							aria-hidden
						/>
						<h2 className="text-caption uppercase text-fg-3">Settings</h2>
						{!settingsOpen && (
							<span className="min-w-0 flex-1 truncate text-right text-sm text-fg-3" title={settingsSummary}>
								{settingsSummary}
							</span>
						)}
					</button>
					{settingsOpen && (
						<div id={settingsId} className="mt-3 space-y-4 pb-1">
							<IntervalField config={p.config} />
							<Switch
								label="Skip tokens analyzed in the last 2 minutes"
								checked={p.config?.skipRecentlyAnalyzed ?? true}
								disabled={!p.config}
								onChange={(v) => void saveConfig({ skipRecentlyAnalyzed: v }, { silent: true })}
							/>
							<EngineLine config={p.config} />
						</div>
					)}
				</div>

				<div className="mt-auto border-t border-line p-4">
					<PlanCard count={p.selected.size} config={p.config} blocked={p.blocked} running={p.running} />
				</div>
			</div>
		</aside>
	);
}
