import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Clock, ListFilter, Radio, RotateCw, ScanSearch, SearchX, Sparkles, Sprout, Star, Zap } from "lucide-react";
import { extractMint, shortMint } from "../../shared/analysis";
import type { MarketTab, MarketToken } from "../../shared/types";
import { PageHeader, SetupBanner } from "../components/shell/chrome";
import { TokenInspector } from "../components/tokens/TokenInspector";
import { TokenTable, TokenTableSkeleton, useTier } from "../components/tokens/TokenTable";
import { FilterChip, FilterInput, UpdatedAgo, useNow } from "../components/tokens/toolbar";
import {
	applyFilters,
	copyMint,
	DEFAULT_SORT,
	EMPTY_LIST,
	EMPTY_TOKENS,
	FILTERS,
	FIRST_DIR,
	loadAutoRefresh,
	MARKET_TABS,
	saveAutoRefresh,
	sortTokens,
	TAB_LABEL,
	watchlistTokens,
	type AutoRefresh,
	type FilterId,
	type Sort,
	type SortKey,
	type TokensTab,
} from "../components/tokens/model";
import { Button, IconButton } from "../components/ui/Button";
import { Banner, EmptyState, ErrorState, Kbd, MOD, Segmented } from "../components/ui/primitives";
import { cn } from "../lib/cn";
import { ago, integer, plural } from "../lib/format";
import { useKeys } from "../lib/hotkeys";
import { useApp } from "../lib/store";

const TAB_ORDER: readonly TokensTab[] = ["all", "live", "new", "graduated", "watchlist"];

const FILTER_ICON: Record<FilterId, JSX.Element> = {
	notAnalyzed: <Sparkles className="size-3.5" />,
	live: <Radio className="size-3.5" />,
	onCurve: <Sprout className="size-3.5" />,
	young: <Clock className="size-3.5" />,
};

const AUTO_OPTIONS = [
	{ value: "off", label: "Off", title: "Do not refresh automatically" },
	{ value: "30", label: "30s", title: "Refresh every 30 seconds while this screen is open" },
	{ value: "60", label: "60s", title: "Refresh every 60 seconds while this screen is open" },
] as const;

const EMPTY_TAB_COPY: Record<MarketTab, { title: string; description: string }> = {
	all: { title: "No tokens right now", description: "pump.studio returned an empty list. Try again in a moment." },
	live: { title: "Nobody is live right now", description: "Tokens that are livestreaming show up here." },
	new: { title: "No new tokens yet", description: "Freshly created tokens show up here." },
	graduated: { title: "No graduated tokens", description: "Tokens that completed their bonding curve show up here." },
};

function latestFetch(market: ReturnType<typeof useApp.getState>["market"]): number | undefined {
	let at: number | undefined;
	for (const t of MARKET_TABS) {
		const f = market[t]?.fetchedAt;
		if (f && (!at || f > at)) at = f;
	}
	return at;
}

export function TokensView({ active }: { active: boolean }) {
	const [tab, setTab] = useState<TokensTab>("all");
	const [filters, setFilters] = useState<ReadonlySet<FilterId>>(() => new Set());
	const [query, setQuery] = useState("");
	const [sort, setSort] = useState<Sort>(DEFAULT_SORT);
	const [cursor, setCursor] = useState<string | null>(null);
	const [auto, setAuto] = useState<AutoRefresh>(loadAutoRefresh);

	const market = useApp((s) => s.market);
	const fetchMarket = useApp((s) => s.fetchMarket);
	const watchlist = useApp((s) => s.config?.prefs.watchlist ?? EMPTY_LIST);
	const tokenMeta = useApp((s) => s.tokenMeta);
	const analyzed = useApp((s) => s.analyzed);
	const inspectMint = useApp((s) => s.inspectMint);
	const setInspect = useApp((s) => s.setInspect);
	const openAnalyze = useApp((s) => s.openAnalyze);
	const toggleWatch = useApp((s) => s.toggleWatch);
	const refreshTick = useApp((s) => s.refreshTick);

	const scrollRef = useRef<HTMLDivElement>(null);
	const inputRef = useRef<HTMLInputElement>(null);
	const tier = useTier(scrollRef);
	const now = useNow(30_000, active);

	/* ---- Data ------------------------------------------------------------- */

	const isWatch = tab === "watchlist";
	const slice = isWatch ? market.all : market[tab];
	const fetchedAt = isWatch ? latestFetch(market) : slice?.fetchedAt;
	const loading = isWatch ? MARKET_TABS.some((t) => market[t]?.loading) : !!slice?.loading;
	const error = slice?.error;

	const baseTokens = useMemo(
		() => (isWatch ? watchlistTokens(watchlist, market, tokenMeta) : (market[tab as MarketTab]?.tokens ?? EMPTY_TOKENS)),
		[isWatch, watchlist, market, tokenMeta, tab],
	);
	const filtered = useMemo(() => applyFilters(baseTokens, filters, query, analyzed, now), [baseTokens, filters, query, analyzed, now]);
	const rows = useMemo(() => sortTokens(filtered, sort), [filtered, sort]);

	const load = useCallback(
		(force: boolean) => {
			if (tab !== "watchlist") {
				void fetchMarket(tab, { force });
				return;
			}
			// Watchlist rows come from every cached slice: refresh those (and "all").
			const m = useApp.getState().market;
			for (const t of MARKET_TABS) if (t === "all" || m[t]) void fetchMarket(t, { force });
		},
		[tab, fetchMarket],
	);

	// First activation and tab switches (cached slices younger than 30s are reused).
	useEffect(() => {
		if (active) load(false);
	}, [active, load]);

	// Global refresh command (Ctrl+R, menu).
	const lastTick = useRef(refreshTick);
	useEffect(() => {
		if (refreshTick === lastTick.current) return;
		lastTick.current = refreshTick;
		if (active) load(true);
	}, [refreshTick, active, load]);

	// Auto-refresh only while this screen is shown and the window is visible.
	useEffect(() => {
		if (!active || auto === "off") return;
		const id = setInterval(() => {
			if (document.visibilityState === "visible") load(true);
		}, Number(auto) * 1000);
		return () => clearInterval(id);
	}, [active, auto, load]);

	/* ---- Derived UI state ---------------------------------------------------- */

	const typedMint = useMemo(() => extractMint(query), [query]);
	const mintOutsideList = !!typedMint && !baseTokens.some((t) => t.mint === typedMint);
	const filtersOn = filters.size > 0 || query.trim().length > 0;
	const hidden = baseTokens.length - rows.length;
	const selected = inspectMint ?? cursor;

	const tabCount = (t: TokensTab): number | undefined => {
		if (t === "watchlist") return watchlist.length;
		const s = market[t];
		return s?.fetchedAt ? s.tokens.length : undefined;
	};

	const inspectToken = useMemo<MarketToken | undefined>(() => {
		if (!inspectMint) return undefined;
		const inList = baseTokens.find((t) => t.mint === inspectMint);
		if (inList) return inList;
		for (const t of MARKET_TABS) {
			const hit = market[t]?.tokens.find((x) => x.mint === inspectMint);
			if (hit) return hit;
		}
		return undefined;
	}, [inspectMint, baseTokens, market]);

	/* ---- Actions --------------------------------------------------------------- */

	const open = useCallback(
		(mint: string) => {
			setCursor(mint);
			setInspect(mint);
		},
		[setInspect],
	);

	const analyze = useCallback((mint: string) => openAnalyze(mint), [openAnalyze]);

	const onSort = useCallback((key: SortKey) => {
		setSort((s) => (s.key === key ? { key, dir: s.dir === "asc" ? "desc" : "asc" } : { key, dir: FIRST_DIR[key] }));
	}, []);

	const toggleFilter = (id: FilterId) =>
		setFilters((prev) => {
			const next = new Set(prev);
			if (next.has(id)) next.delete(id);
			else next.add(id);
			return next;
		});

	const clearFilters = () => {
		setFilters(new Set());
		setQuery("");
	};

	const scrollToMint = (mint: string) => {
		requestAnimationFrame(() => {
			const el = scrollRef.current?.querySelector<HTMLElement>(`tr[data-mint="${CSS.escape(mint)}"]`);
			el?.scrollIntoView({ block: "nearest" });
		});
	};

	const move = (delta: number) => {
		if (rows.length === 0) return;
		const idx = selected ? rows.findIndex((t) => t.mint === selected) : -1;
		const nextIdx = idx < 0 ? (delta > 0 ? 0 : rows.length - 1) : Math.max(0, Math.min(rows.length - 1, idx + delta));
		const next = rows[nextIdx]!.mint;
		setCursor(next);
		if (inspectMint) setInspect(next);
		scrollToMint(next);
	};

	useKeys((e) => {
		const k = e.key;
		const target = selected;
		if (k === "j" || k === "ArrowDown") return move(1), true;
		if (k === "k" || k === "ArrowUp") return move(-1), true;
		if (k === "Escape" && inspectMint) return setInspect(null), true;
		if (k === "Enter") {
			const el = e.target as HTMLElement | null;
			if (el?.closest("button, a, [role='radio'], [role='switch']")) return;
			if (!target) return;
			open(target);
			return true;
		}
		if (!target) return;
		if (k === "a") return openAnalyze(target), true;
		if (k === "w") return toggleWatch(target), true;
		if (k === "c") return void copyMint(target), true;
	}, active);

	const onFilterEnter = () => {
		if (typedMint && mintOutsideList) {
			openAnalyze(typedMint);
			return;
		}
		const first = rows[0];
		if (first) {
			open(first.mint);
			scrollToMint(first.mint);
		}
	};

	/* ---- Body state ------------------------------------------------------------------ */

	const knownMintMeta = typedMint ? tokenMeta[typedMint] : undefined;
	const initialLoading = !fetchedAt && !error && (loading || !slice) && !(isWatch && watchlist.length === 0);
	let body: JSX.Element;
	if (isWatch && watchlist.length === 0) {
		body = (
			<EmptyState
				icon={<Star className="size-5" />}
				title="Your watchlist is empty"
				description="Select the star on any token, or press W on a selected row, to keep it here."
				action={<Button onClick={() => setTab("all")}>Browse all tokens</Button>}
			/>
		);
	} else if (initialLoading) {
		body = <TokenTableSkeleton tier={tier} />;
	} else if (error && baseTokens.length === 0) {
		body = <ErrorState title="Could not load tokens" message={error} onRetry={() => load(true)} />;
	} else if (baseTokens.length === 0) {
		const copy = EMPTY_TAB_COPY[tab as MarketTab];
		body = (
			<EmptyState
				icon={<ListFilter className="size-5" />}
				title={copy.title}
				description={copy.description}
				action={
					<Button icon={<RotateCw className="size-4" aria-hidden />} onClick={() => load(true)} loading={loading}>
						Refresh
					</Button>
				}
			/>
		);
	} else if (rows.length === 0 && typedMint && mintOutsideList) {
		body = (
			<EmptyState
				icon={<ScanSearch className="size-5" />}
				title={knownMintMeta?.symbol ? `$${knownMintMeta.symbol} is not in this list` : "This mint is not in this list"}
				description={
					<>
						<span className="font-mono text-sm" title={typedMint}>
							{shortMint(typedMint, 6, 6)}
						</span>{" "}
						{tab === "all" ? "is not in the current list" : `is not in the ${TAB_LABEL[tab]} tab`}. You can still look it up and
						analyze it.
					</>
				}
				action={
					<>
						<Button onClick={() => open(typedMint)}>Show details</Button>
						<Button variant="primary" kbd="↵" icon={<Zap className="size-4" aria-hidden />} onClick={() => openAnalyze(typedMint)}>
							Analyze
						</Button>
					</>
				}
			/>
		);
	} else if (rows.length === 0) {
		body = (
			<EmptyState
				icon={<SearchX className="size-5" />}
				title="No tokens match your filters"
				description={`All ${plural(baseTokens.length, "token")} in this tab are hidden by the current filters.`}
				action={<Button onClick={clearFilters}>Clear filters</Button>}
			/>
		);
	} else {
		body = (
			<TokenTable
				tokens={rows}
				tier={tier}
				sort={sort}
				onSort={onSort}
				selectedMint={selected}
				analyzed={analyzed}
				now={now}
				onOpen={open}
				onAnalyze={analyze}
			/>
		);
	}

	const showStale = !!error && baseTokens.length > 0 && !(isWatch && watchlist.length === 0);

	return (
		<div className="flex min-h-0 flex-1 flex-col">
			<PageHeader
				title="Tokens"
				description="Browse pump.fun tokens and pick one to analyze"
				actions={
					<>
						<UpdatedAgo at={fetchedAt} loading={loading} active={active} />
						<span aria-hidden className="mx-1 hidden h-4 w-px bg-line lg:block" />
						<div className="flex items-center gap-2">
							<span className="hidden text-sm text-fg-3 xl:inline">
								Auto-refresh
							</span>
							<Segmented size="sm" label="Auto-refresh" value={auto} options={AUTO_OPTIONS} onChange={(v) => (setAuto(v), saveAutoRefresh(v))} />
						</div>
						<IconButton label={`Refresh (${MOD}+R)`} onClick={() => load(true)} aria-busy={loading || undefined}>
							<RotateCw className={cn("size-4", loading && "animate-spin")} aria-hidden />
						</IconButton>
					</>
				}
			>
				<div className="flex flex-wrap items-center gap-2">
					<Segmented
						label="Token list"
						value={tab}
						onChange={setTab}
						options={TAB_ORDER.map((t) => {
							const count = tabCount(t);
							return {
								value: t,
								label: (
									<>
										{t === "watchlist" && <Star className="size-3.5" aria-hidden />}
										{TAB_LABEL[t]}
										{count != null && <span className="num text-sm font-normal text-fg-3">{integer(count)}</span>}
									</>
								),
							};
						})}
					/>
					<span aria-hidden className="mx-1 hidden h-5 w-px bg-line min-[1340px]:block" />
					<div className="order-last flex w-full flex-wrap items-center gap-1.5 min-[1340px]:order-none min-[1340px]:w-auto" role="group" aria-label="Filters">
						{FILTERS.map((f) => (
							<FilterChip key={f.id} pressed={filters.has(f.id)} onToggle={() => toggleFilter(f.id)} icon={FILTER_ICON[f.id]} title={f.title}>
								{f.id === "notAnalyzed" ? (
									<span>
										Not analyzed<span className="sr-only 2xl:not-sr-only"> by me</span>
									</span>
								) : (
									f.label
								)}
							</FilterChip>
						))}
					</div>
					<FilterInput
						ref={inputRef}
						value={query}
						onChange={setQuery}
						onEnter={onFilterEnter}
						className="ml-auto min-w-[200px] max-w-[300px] flex-1"
					/>
				</div>
			</PageHeader>

			<div className="flex min-h-0 flex-1">
				<div className="flex min-w-0 flex-1 flex-col">
					<SetupBanner compact className="mx-5 mt-3" />
					{showStale && (
						<Banner
							tone="warn"
							className="mx-5 mt-3"
							title="Could not refresh the list"
							action={
								<Button size="sm" icon={<RotateCw className="size-3.5" aria-hidden />} onClick={() => load(true)} loading={loading}>
									Try again
								</Button>
							}
						>
							{error}. Showing data from {ago(fetchedAt)}.
						</Banner>
					)}
					{typedMint && mintOutsideList && rows.length > 0 && (
						<div className="mx-5 mt-3 flex items-center gap-3 rounded-lg border border-info/25 bg-info/5 px-3 py-2" role="status">
							<ScanSearch className="size-4 shrink-0 text-info" aria-hidden />
							<div className="min-w-0 flex-1 truncate text-base text-fg-2">
								<span className="font-medium text-fg">
									{knownMintMeta?.symbol ? `$${knownMintMeta.symbol}` : "This mint"}
								</span>{" "}
								<span className="font-mono text-sm text-fg-3" title={typedMint}>
									{shortMint(typedMint, 6, 6)}
								</span>{" "}
								is not in this list.
							</div>
							<Button size="sm" variant="ghost" onClick={() => open(typedMint)}>
								Show details
							</Button>
							<Button size="sm" variant="primary" icon={<Zap className="size-3.5" aria-hidden />} onClick={() => openAnalyze(typedMint)}>
								Analyze {shortMint(typedMint)}
							</Button>
						</div>
					)}

					<div ref={scrollRef} className="mt-0 min-h-0 flex-1 overflow-y-auto overflow-x-hidden">
						{body}
					</div>

					{/* Footer */}
					<div className="flex h-9 shrink-0 items-center gap-3 border-t border-line bg-surface-1 px-5 text-sm text-fg-3">
						<div className="num flex min-w-0 items-center gap-1.5 truncate">
							{baseTokens.length > 0 || fetchedAt ? (
								<>
									<span className="text-fg-2">{plural(baseTokens.length, "token")}</span>
									{hidden > 0 && (
										<>
											<span aria-hidden>·</span>
											<span>{integer(hidden)} hidden by filters</span>
										</>
									)}
									{filtersOn && (
										<>
											<span aria-hidden>·</span>
											<button type="button" onClick={clearFilters} className="rounded text-fg-2 underline-offset-2 hover:text-fg hover:underline">
												Clear filters
											</button>
										</>
									)}
								</>
							) : (
								<span>{loading ? "Loading tokens" : "No data yet"}</span>
							)}
						</div>
						{tier !== "narrow" && (
							<div className="ml-auto flex shrink-0 items-center gap-3" aria-hidden>
								<span className="flex items-center gap-1">
									<Kbd>J</Kbd>
									<Kbd>K</Kbd>
									<span className="ml-0.5">move</span>
								</span>
								<span className="flex items-center gap-1">
									<Kbd>Enter</Kbd>
									<span className="ml-0.5">details</span>
								</span>
								<span className="flex items-center gap-1">
									<Kbd>A</Kbd>
									<span className="ml-0.5">analyze</span>
								</span>
								{tier === "full" && (
									<span className="flex items-center gap-1">
										<Kbd>W</Kbd>
										<span className="ml-0.5">watch</span>
									</span>
								)}
							</div>
						)}
					</div>
				</div>

				{inspectMint && <TokenInspector mint={inspectMint} token={inspectToken} onClose={() => setInspect(null)} />}
			</div>
		</div>
	);
}
