import { memo, useEffect, useState, type ReactNode, type RefObject } from "react";
import { ArrowDown, ArrowUp, ChevronsUpDown, GraduationCap, Radio, Zap } from "lucide-react";
import type { MarketToken } from "../../../shared/types";
import { cn } from "../../lib/cn";
import { absoluteTime, ageShort, ago, usdCompact } from "../../lib/format";
import type { MyAnalysis } from "../../lib/store";
import { CopyMint, SentimentBadge, TokenAvatar, TokenStatus, WatchButton } from "../token/widgets";
import { Button, IconButton } from "../ui/Button";
import { Skeleton } from "../ui/primitives";
import { marketCap, type Sort, type SortKey } from "./model";

/* ---- Responsive columns ------------------------------------------------ */

/** Layout tier from the table container's width (the inspector can take 380px). */
export type Tier = "full" | "mid" | "narrow";

export function useTier(ref: RefObject<HTMLElement>): Tier {
	const [tier, setTier] = useState<Tier>("full");
	useEffect(() => {
		const el = ref.current;
		if (!el) return;
		const measure = () => {
			const w = el.clientWidth;
			setTier(w >= 800 ? "full" : w >= 620 ? "mid" : "narrow");
		};
		measure();
		const ro = new ResizeObserver(measure);
		ro.observe(el);
		return () => ro.disconnect();
	}, [ref]);
	return tier;
}

const CELL = "px-[var(--cell-px)] first:pl-5 last:pr-5";

function Cols({ tier }: { tier: Tier }) {
	return (
		<colgroup>
			<col style={{ width: 60 }} />
			<col />
			<col style={{ width: 64 }} />
			<col style={{ width: 116 }} />
			{tier === "full" && <col style={{ width: 128 }} />}
			{tier !== "narrow" && <col style={{ width: 124 }} />}
			<col style={{ width: tier === "narrow" ? 88 : 140 }} />
		</colgroup>
	);
}

/* ---- Header ------------------------------------------------------------ */

function Th({
	children,
	align = "left",
	sortKey,
	sort,
	onSort,
	className,
}: {
	children: ReactNode;
	align?: "left" | "right";
	sortKey?: SortKey;
	sort?: Sort;
	onSort?: (k: SortKey) => void;
	className?: string;
}) {
	const activeSort = sortKey && sort?.key === sortKey ? sort.dir : null;
	const base = cn(
		"sticky top-0 z-10 h-9 whitespace-nowrap border-b border-line bg-surface-1 text-caption font-medium uppercase text-fg-3",
		CELL,
		align === "right" ? "text-right" : "text-left",
		className,
	);
	if (!sortKey || !onSort) {
		return (
			<th scope="col" className={base}>
				{children}
			</th>
		);
	}
	const Icon = activeSort === "asc" ? ArrowUp : activeSort === "desc" ? ArrowDown : ChevronsUpDown;
	return (
		<th scope="col" aria-sort={activeSort === "asc" ? "ascending" : activeSort === "desc" ? "descending" : "none"} className={base}>
			<button
				type="button"
				onClick={() => onSort(sortKey)}
				className={cn(
					"focus-inset -mx-1 inline-flex h-6 items-center gap-1 rounded px-1 uppercase transition-colors hover:text-fg",
					activeSort && "text-fg-2",
					align === "right" && "flex-row-reverse",
				)}
			>
				{children}
				<Icon className={cn("size-3 shrink-0", !activeSort && "opacity-50")} aria-hidden />
			</button>
		</th>
	);
}

function Head({ tier, sort, onSort }: { tier: Tier; sort?: Sort; onSort?: (k: SortKey) => void }) {
	return (
		<thead>
			<tr>
				<Th align="right">#</Th>
				<Th sortKey="name" sort={sort} onSort={onSort}>
					Token
				</Th>
				<Th align="right" sortKey="age" sort={sort} onSort={onSort}>
					Age
				</Th>
				<Th align="right" sortKey="mcap" sort={sort} onSort={onSort}>
					Market cap
				</Th>
				{tier === "full" && <Th>Status</Th>}
				{tier !== "narrow" && <Th>My call</Th>}
				<Th align="right">
					<span className="sr-only">Actions</span>
				</Th>
			</tr>
		</thead>
	);
}

/** Icon-only status for narrow layouts (title + screen reader text, never color alone). */
function StatusIcons({ live, graduated }: { live?: boolean; graduated?: boolean }) {
	const box = "inline-flex size-5 shrink-0 items-center justify-center rounded border";
	return (
		<span className="flex shrink-0 gap-1">
			{live && (
				<span className={cn(box, "border-neg/25 bg-neg/10 text-neg")} title="Livestreaming now">
					<Radio className="size-3" aria-hidden />
					<span className="sr-only">Live</span>
				</span>
			)}
			{graduated && (
				<span className={cn(box, "border-info/25 bg-info/10 text-info")} title="Graduated: bonding curve complete">
					<GraduationCap className="size-3" aria-hidden />
					<span className="sr-only">Graduated</span>
				</span>
			)}
		</span>
	);
}

/* ---- Row --------------------------------------------------------------- */

const Row = memo(function Row({
	token: t,
	rank,
	tier,
	selected,
	mine,
	now,
	onOpen,
	onAnalyze,
}: {
	token: MarketToken;
	rank: number;
	tier: Tier;
	selected: boolean;
	mine?: MyAnalysis;
	now: number;
	onOpen: (mint: string) => void;
	onAnalyze: (mint: string) => void;
}) {
	const label = t.symbol ? `$${t.symbol}` : "Unknown token";
	const status = <TokenStatus live={t.is_currently_live} graduated={t.complete === true} />;
	return (
		<tr
			data-mint={t.mint}
			aria-current={selected ? "true" : undefined}
			onClick={() => onOpen(t.mint)}
			className={cn("group h-row scroll-mt-9 transition-colors duration-fast", selected ? "bg-surface-3" : "focus-within:bg-surface-2 hover:bg-surface-2")}
		>
			<td className={cn("relative border-b border-line text-right", CELL)}>
				{selected && <span aria-hidden className="absolute inset-y-0 left-0 w-0.5 bg-brand" />}
				<span className="num text-sm text-fg-3">{rank}</span>
			</td>
			<td className={cn("relative border-b border-line", CELL)}>
				<div className="flex min-w-0 items-center gap-1.5">
					<button
						type="button"
						onClick={(e) => {
							e.stopPropagation();
							onOpen(t.mint);
						}}
						aria-label={`Show details for ${label}${t.name ? `, ${t.name}` : ""}`}
						className="focus-inset -ml-1 flex min-w-0 items-center gap-2.5 rounded px-1 py-0.5 text-left"
					>
						<TokenAvatar src={t.image_uri} symbol={t.symbol} mint={t.mint} size={28} progress={t.complete ? 100 : undefined} />
						<span className="max-w-[9rem] shrink-0 truncate font-medium text-fg" title={t.symbol ? `$${t.symbol}` : undefined}>
							{t.symbol ? `$${t.symbol}` : <span className="text-fg-3">Unknown</span>}
						</span>
						{t.name && (
							<span className="min-w-0 truncate text-fg-3" title={t.name}>
								{t.name}
							</span>
						)}
					</button>
					{tier !== "full" && (t.is_currently_live || t.complete) && (
						<StatusIcons live={t.is_currently_live} graduated={t.complete === true} />
					)}
				</div>
				{/* Overlays the end of the cell on hover/focus, so it never takes space from the name. */}
				<span
					className={cn(
						"pointer-events-none absolute inset-y-0 right-1 flex items-center pl-2 opacity-0 transition-opacity duration-fast",
						"group-focus-within:pointer-events-auto group-focus-within:opacity-100 group-hover:pointer-events-auto group-hover:opacity-100",
						selected ? "bg-surface-3" : "bg-surface-2",
					)}
				>
					<CopyMint mint={t.mint} />
				</span>
			</td>
			<td className={cn("border-b border-line text-right", CELL)}>
				<span className="num text-fg-2" title={t.created_timestamp ? absoluteTime(t.created_timestamp) : undefined}>
					{ageShort(t.created_timestamp, now)}
				</span>
			</td>
			<td className={cn("border-b border-line text-right", CELL)}>
				<span className="num font-medium text-fg">{usdCompact(marketCap(t))}</span>
			</td>
			{tier === "full" && (
				<td className={cn("border-b border-line", CELL)}>
					<div className="flex gap-1">{status}</div>
				</td>
			)}
			{tier !== "narrow" && (
				<td className={cn("border-b border-line", CELL)}>
					{mine ? (
						<span title={`Your call ${ago(mine.at, now)}${mine.count > 1 ? `, analyzed ${mine.count} times` : ""}`}>
							<SentimentBadge value={mine.sentiment} score={mine.score} />
						</span>
					) : (
						<span className="text-fg-3" aria-label="Not analyzed">
							—
						</span>
					)}
				</td>
			)}
			<td className={cn("border-b border-line", CELL)}>
				<div className="flex items-center justify-end gap-1">
					<WatchButton mint={t.mint} />
					{tier === "narrow" ? (
						<IconButton
							size="sm"
							label={`Analyze ${label}`}
							onClick={(e) => {
								e.stopPropagation();
								onAnalyze(t.mint);
							}}
						>
							<Zap className="size-4" aria-hidden />
						</IconButton>
					) : (
						<Button
							size="sm"
							variant="secondary"
							aria-label={`Analyze ${label}`}
							onClick={(e) => {
								e.stopPropagation();
								onAnalyze(t.mint);
							}}
							className={cn(
								"transition-opacity",
								!selected && "opacity-0 focus-visible:opacity-100 group-hover:opacity-100 group-focus-within:opacity-100",
							)}
						>
							Analyze
						</Button>
					)}
				</div>
			</td>
		</tr>
	);
});

/* ---- Table ------------------------------------------------------------- */

export function TokenTable({
	tokens,
	tier,
	sort,
	onSort,
	selectedMint,
	analyzed,
	now,
	onOpen,
	onAnalyze,
}: {
	tokens: readonly MarketToken[];
	tier: Tier;
	sort: Sort;
	onSort: (k: SortKey) => void;
	selectedMint: string | null;
	analyzed: Record<string, MyAnalysis>;
	now: number;
	onOpen: (mint: string) => void;
	onAnalyze: (mint: string) => void;
}) {
	return (
		<table className="w-full table-fixed border-separate border-spacing-0 text-base">
			<caption className="sr-only">Tokens. Select a row to see details.</caption>
			<Cols tier={tier} />
			<Head tier={tier} sort={sort} onSort={onSort} />
			<tbody>
				{tokens.map((t, i) => (
					<Row
						key={t.mint}
						token={t}
						rank={i + 1}
						tier={tier}
						selected={t.mint === selectedMint}
						mine={analyzed[t.mint]}
						now={now}
						onOpen={onOpen}
						onAnalyze={onAnalyze}
					/>
				))}
			</tbody>
		</table>
	);
}

/** Loading placeholder with the same columns and row height as the table. */
export function TokenTableSkeleton({ tier, rows = 14 }: { tier: Tier; rows?: number }) {
	return (
		<table className="w-full table-fixed border-separate border-spacing-0 text-base" aria-busy="true">
			<caption className="sr-only">Loading tokens</caption>
			<Cols tier={tier} />
			<Head tier={tier} />
			<tbody>
				{Array.from({ length: rows }, (_, i) => (
					<tr key={i} className="h-row">
						<td className={cn("border-b border-line", CELL)}>
							<Skeleton className="ml-auto h-3 w-4" />
						</td>
						<td className={cn("border-b border-line", CELL)}>
							<div className="flex items-center gap-2.5">
								<Skeleton className="size-7 shrink-0 rounded-full" />
								<Skeleton className="h-3 w-14" />
								<div aria-hidden className="skeleton skeleton-shimmer h-3" style={{ width: 70 + ((i * 37) % 90) }} />
							</div>
						</td>
						<td className={cn("border-b border-line", CELL)}>
							<Skeleton className="ml-auto h-3 w-7" />
						</td>
						<td className={cn("border-b border-line", CELL)}>
							<Skeleton className="ml-auto h-3 w-14" />
						</td>
						{tier === "full" && (
							<td className={cn("border-b border-line", CELL)}>{i % 3 === 0 && <Skeleton className="h-5 w-16 rounded" />}</td>
						)}
						{tier !== "narrow" && (
							<td className={cn("border-b border-line", CELL)}>
								<Skeleton className="h-3 w-3" />
							</td>
						)}
						<td className={cn("border-b border-line", CELL)}>
							<Skeleton className="ml-auto size-5 rounded" />
						</td>
					</tr>
				))}
			</tbody>
		</table>
	);
}
