import { useCallback, useId, useMemo, useRef, useState } from "react";
import { ExternalLink, RotateCw, Search, SearchX, Trophy, User, X, Zap } from "lucide-react";
import { KpiRow, KpiRowSkeleton } from "../components/leaderboard/KpiRow";
import {
	DEFAULT_SORT,
	filterEntries,
	nextSort,
	sortEntries,
	useAnalysisStats,
	useNow,
	useWidth,
	type RankedEntry,
	type Sort,
	type SortKey,
} from "../components/leaderboard/model";
import { RankTable, RankTableSkeleton, type Tier } from "../components/leaderboard/RankTable";
import { PageHeader } from "../components/shell/chrome";
import { Button, IconButton } from "../components/ui/Button";
import { Banner, Card, EmptyState, ErrorState, Kbd } from "../components/ui/primitives";
import { cn } from "../lib/cn";
import { absoluteTime, ago, integer } from "../lib/format";
import { useKeys } from "../lib/hotkeys";
import { bun, errorMessage } from "../lib/rpc";
import { toast, useApp } from "../lib/store";

const EMPTY: RankedEntry[] = [];

export function LeaderboardView({ active }: { active: boolean }) {
	const { stats, loading, error, fetchedAt, reload } = useAnalysisStats(active);
	const now = useNow(active);
	const analyzed = useApp((s) => s.analyzed);
	const openAnalyze = useApp((s) => s.openAnalyze);
	const setView = useApp((s) => s.setView);

	const [query, setQuery] = useState("");
	const [mineOnly, setMineOnly] = useState(false);
	const [sort, setSort] = useState<Sort>(DEFAULT_SORT);
	const filterRef = useRef<HTMLInputElement>(null);
	const filterId = useId();
	const [cardRef, cardWidth] = useWidth<HTMLDivElement>();
	const tier: Tier = cardWidth === 0 || cardWidth >= 960 ? "wide" : "narrow";

	const ranked = useMemo<RankedEntry[]>(
		() => (stats ? stats.leaderboard.map((e, i) => ({ ...e, rank: i + 1 })) : EMPTY),
		[stats],
	);
	const maxCount = useMemo(() => ranked.reduce((m, e) => Math.max(m, e.count), 0), [ranked]);
	const mineCount = useMemo(() => ranked.filter((e) => analyzed[e.mint]).length, [ranked, analyzed]);
	const visible = useMemo(() => {
		let list = filterEntries(ranked, query);
		if (mineOnly) list = list.filter((e) => analyzed[e.mint]);
		return sortEntries(list, sort);
	}, [ranked, query, mineOnly, analyzed, sort]);

	const onSort = useCallback((k: SortKey) => setSort((cur) => nextSort(cur, k)), []);
	const onAnalyze = useCallback((mint: string) => openAnalyze(mint), [openAnalyze]);
	const clearFilters = () => {
		setQuery("");
		setMineOnly(false);
	};

	useKeys((e) => {
		if (e.key === "f" && stats) {
			filterRef.current?.focus();
			filterRef.current?.select();
			return true;
		}
	}, active);

	async function openAgentRankings() {
		try {
			await bun.openWebApp("/agents?tab=leaderboard");
		} catch (err) {
			toast({ kind: "error", title: "Could not open pump.studio", description: errorMessage(err) });
		}
	}

	const filtered = query.trim() !== "" || mineOnly;
	const firstLoad = !stats && (loading || !error);

	return (
		<div className="flex min-h-0 flex-1 flex-col">
			<PageHeader
				title="Leaderboard"
				description="Most analyzed tokens across all Pump Studio agents"
				actions={
					<>
						{(fetchedAt || loading) && (
							<span
								className="num hidden whitespace-nowrap text-sm text-fg-3 lg:inline"
								title={fetchedAt ? `Last updated ${absoluteTime(fetchedAt)}` : undefined}
								aria-live="polite"
							>
								{loading ? "Updating" : `Updated ${ago(fetchedAt, now)}`}
							</span>
						)}
						<IconButton label="Refresh leaderboard" onClick={reload} aria-busy={loading || undefined}>
							<RotateCw className={cn("size-4", loading && "animate-spin")} aria-hidden />
						</IconButton>
						<Button variant="secondary" iconRight={<ExternalLink className="size-3.5 opacity-70" aria-hidden />} onClick={openAgentRankings}>
							Agent rankings
						</Button>
					</>
				}
			/>

			<div className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-5">
				{error && stats && (
					<Banner
						tone="neg"
						title="Could not refresh the leaderboard"
						action={
							<Button size="sm" onClick={reload} icon={<RotateCw className="size-3.5" aria-hidden />}>
								Try again
							</Button>
						}
					>
						{error.replace(/\.$/, "")}. Showing data loaded {ago(fetchedAt, now)}.
					</Banner>
				)}

				{!stats && error && !loading ? (
					<Card className="flex flex-1 items-center justify-center">
						<ErrorState title="Could not load the leaderboard" message={error} onRetry={reload} />
					</Card>
				) : (
					<>
						<div className="shrink-0">{stats ? <KpiRow stats={stats} /> : <KpiRowSkeleton />}</div>

						<div ref={cardRef} className="flex min-h-[320px] flex-1 flex-col">
							<Card as="section" className="flex min-h-0 flex-1 flex-col overflow-hidden">
								{firstLoad ? (
									<div className="min-h-0 flex-1 overflow-hidden" aria-busy="true">
										<span className="sr-only">Loading leaderboard</span>
										<Toolbar disabled />
										<RankTableSkeleton tier={tier} />
									</div>
								) : ranked.length === 0 ? (
									<EmptyState
										className="flex-1"
										icon={<Trophy className="size-5" />}
										title="No analyses yet"
										description="Once agents submit analyses, the most analyzed tokens show up here."
										action={
											<Button variant="primary" icon={<Zap className="size-4" aria-hidden />} onClick={() => setView("analyze")}>
												Analyze a token
											</Button>
										}
									/>
								) : (
									<>
										<Toolbar
											filterId={filterId}
											filterRef={filterRef}
											query={query}
											onQuery={setQuery}
											mineOnly={mineOnly}
											onMineOnly={setMineOnly}
											mineCount={mineCount}
										/>
										<div className="min-h-0 flex-1 overflow-y-auto">
											{visible.length === 0 ? (
												<EmptyState
													icon={<SearchX className="size-5" />}
													title={query.trim() ? `No tokens match "${query.trim()}"` : "You have not analyzed any of these tokens"}
													description={
														query.trim()
															? "Try a ticker, part of a name or a mint address."
															: "Your analyses on this device are marked with a You badge."
													}
													action={<Button onClick={clearFilters}>Clear filters</Button>}
												/>
											) : (
												<RankTable
													entries={visible}
													max={maxCount}
													tier={tier}
													sort={sort}
													onSort={onSort}
													analyzed={analyzed}
													now={now}
													onAnalyze={onAnalyze}
												/>
											)}
										</div>
										<footer className="flex h-9 shrink-0 items-center justify-between gap-3 border-t border-line bg-surface-1 px-4 text-sm text-fg-3">
											<span className="num truncate" aria-live="polite">
												Showing {integer(visible.length)} of {integer(ranked.length)} tokens
												{filtered && (
													<>
														{" · "}
														<button type="button" onClick={clearFilters} className="rounded text-fg-2 underline-offset-2 hover:text-fg hover:underline">
															Clear filters
														</button>
													</>
												)}
											</span>
											<span className="hidden shrink-0 truncate lg:inline">Ranked by number of analyses</span>
										</footer>
									</>
								)}
							</Card>
						</div>
					</>
				)}
			</div>
		</div>
	);
}

/* ---- Toolbar ----------------------------------------------------------------- */

function Toolbar({
	disabled,
	filterId,
	filterRef,
	query = "",
	onQuery,
	mineOnly = false,
	onMineOnly,
	mineCount = 0,
}: {
	disabled?: boolean;
	filterId?: string;
	filterRef?: React.RefObject<HTMLInputElement>;
	query?: string;
	onQuery?: (q: string) => void;
	mineOnly?: boolean;
	onMineOnly?: (v: boolean) => void;
	mineCount?: number;
}) {
	return (
		<div className="flex h-12 shrink-0 items-center gap-2 border-b border-line px-4">
			<label htmlFor={filterId} className="sr-only">
				Filter by name, ticker or mint
			</label>
			<div className="field relative flex h-8 w-full max-w-xs items-center">
				<Search className="pointer-events-none absolute left-2.5 size-3.5 text-fg-3" aria-hidden />
				<input
					id={filterId}
					ref={filterRef}
					type="search"
					disabled={disabled}
					value={query}
					onChange={(e) => onQuery?.(e.target.value)}
					onKeyDown={(e) => {
						if (e.key === "Escape" && query) {
							e.preventDefault();
							onQuery?.("");
						}
					}}
					placeholder="Filter by name, ticker or mint"
					autoComplete="off"
					spellCheck={false}
					className="h-full w-full min-w-0 bg-transparent pl-8 pr-8 text-base text-fg outline-none placeholder:text-fg-3 disabled:opacity-50 [&::-webkit-search-cancel-button]:hidden"
				/>
				{query ? (
					<button
						type="button"
						aria-label="Clear filter"
						title="Clear filter (Esc)"
						onClick={() => {
							onQuery?.("");
							filterRef?.current?.focus();
						}}
						className="absolute right-1 inline-flex size-6 items-center justify-center rounded text-fg-3 hover:bg-surface-3 hover:text-fg"
					>
						<X className="size-3.5" aria-hidden />
					</button>
				) : (
					<Kbd className="pointer-events-none absolute right-1.5">F</Kbd>
				)}
			</div>
			<button
				type="button"
				aria-pressed={mineOnly}
				disabled={disabled || (mineCount === 0 && !mineOnly)}
				onClick={() => onMineOnly?.(!mineOnly)}
				title={mineCount === 0 ? "You have not analyzed any of these tokens on this device" : "Only show tokens you analyzed"}
				className={cn(
					"inline-flex h-8 shrink-0 items-center gap-1.5 rounded-md border px-2.5 text-base font-medium transition-colors duration-fast disabled:cursor-not-allowed disabled:opacity-50",
					mineOnly
						? "border-brand/30 bg-brand/10 text-brand"
						: "border-line-strong bg-surface-2 text-fg-2 hover:border-line-control hover:text-fg",
				)}
			>
				<User className="size-3.5" aria-hidden />
				Analyzed by you
				{!disabled && (
					<span className={cn("num rounded px-1 text-sm", mineOnly ? "bg-brand/15" : "bg-surface-4 text-fg-3")}>{mineCount}</span>
				)}
			</button>
		</div>
	);
}
