import { memo, useEffect, useRef, useState, type ReactNode } from "react";
import {
	ArrowDown,
	ArrowDownRight,
	ArrowUp,
	ArrowUpRight,
	Check,
	ChevronsUpDown,
	Copy,
	Medal,
	Minus,
	Trophy,
	Zap,
} from "lucide-react";
import { shortMint } from "../../../shared/analysis";
import type { Sentiment } from "../../../shared/types";
import { cn } from "../../lib/cn";
import { ago, integer, titleCase } from "../../lib/format";
import { errorMessage } from "../../lib/rpc";
import { toast, type MyAnalysis } from "../../lib/store";
import { CopyMint, Meter, SENTIMENT_TONE, SentimentBadge, TokenAvatar } from "../token/widgets";
import { Button, IconButton } from "../ui/Button";
import { Badge, Skeleton } from "../ui/primitives";
import { scoreTone, type RankedEntry, type Sort, type SortKey } from "./model";

/** "wide" shows inline bars and full labels; "narrow" fits 800px-ish cards. */
export type Tier = "wide" | "narrow";

const CELL = "px-[var(--cell-px)] first:pl-4 last:pr-4";

function Cols({ tier }: { tier: Tier }) {
	const w = tier === "wide";
	return (
		<colgroup>
			<col style={{ width: w ? 72 : 64 }} />
			<col />
			<col style={{ width: w ? 168 : 112 }} />
			<col style={{ width: w ? 120 : 108 }} />
			<col style={{ width: w ? 128 : 108 }} />
			<col style={{ width: w ? 150 : 108 }} />
			<col style={{ width: w ? 112 : 52 }} />
		</colgroup>
	);
}

/* ---- Header ---------------------------------------------------------------- */

function Th({
	children,
	align = "left",
	sortKey,
	sort,
	onSort,
	title,
}: {
	children: ReactNode;
	align?: "left" | "right";
	sortKey?: SortKey;
	sort?: Sort;
	onSort?: (k: SortKey) => void;
	title?: string;
}) {
	const active = sortKey && sort?.key === sortKey ? sort.dir : null;
	const base = cn(
		"sticky top-0 z-10 h-9 whitespace-nowrap border-b border-line bg-surface-1 text-caption font-medium uppercase text-fg-3",
		CELL,
		align === "right" ? "text-right" : "text-left",
	);
	if (!sortKey || !onSort) {
		return (
			<th scope="col" className={base} title={title}>
				{children}
			</th>
		);
	}
	const Icon = active === "asc" ? ArrowUp : active === "desc" ? ArrowDown : ChevronsUpDown;
	return (
		<th scope="col" aria-sort={active === "asc" ? "ascending" : active === "desc" ? "descending" : "none"} className={base}>
			<button
				type="button"
				onClick={() => onSort(sortKey)}
				title={title}
				className={cn(
					"focus-inset -mx-1 inline-flex h-6 items-center gap-1 rounded px-1 uppercase transition-colors duration-fast hover:text-fg",
					active && "text-fg",
					align === "right" && "flex-row-reverse",
				)}
			>
				{children}
				<Icon className={cn("size-3 shrink-0", !active && "opacity-50")} aria-hidden />
			</button>
		</th>
	);
}

function Head({ sort, onSort }: { sort: Sort; onSort: (k: SortKey) => void }) {
	return (
		<thead>
			<tr>
				<Th align="right" sortKey="rank" sort={sort} onSort={onSort} title="Sort by rank">
					Rank
				</Th>
				<Th>Token</Th>
				<Th align="right" sortKey="count" sort={sort} onSort={onSort} title="Sort by number of analyses">
					Analyses
				</Th>
				<Th title="Sentiment of the most recent analysis">Last call</Th>
				<Th align="right" sortKey="score" sort={sort} onSort={onSort} title="Sort by score of the most recent analysis">
					Last score
				</Th>
				<Th title="Your own latest analysis on this device">You</Th>
				<Th align="right">
					<span className="sr-only">Actions</span>
				</Th>
			</tr>
		</thead>
	);
}

/* ---- Cells ----------------------------------------------------------------- */

const PODIUM: Record<number, { icon: typeof Trophy; color: string; label: string }> = {
	1: { icon: Trophy, color: "text-warn", label: "First place" },
	2: { icon: Medal, color: "text-fg-2", label: "Second place" },
	3: { icon: Medal, color: "text-orange", label: "Third place" },
};

function RankCell({ rank }: { rank: number }) {
	const p = PODIUM[rank];
	if (!p) return <span className="num text-sm text-fg-3">{rank}</span>;
	const Icon = p.icon;
	return (
		<span className="inline-flex items-center justify-end gap-1.5" title={p.label}>
			<Icon className={cn("size-3.5 shrink-0", p.color)} aria-hidden />
			<span className="num text-base font-semibold text-fg">{rank}</span>
			<span className="sr-only">{p.label}</span>
		</span>
	);
}

function CountCell({ count, max, tier }: { count: number; max: number; tier: Tier }) {
	const share = max > 0 ? (count / max) * 100 : 0;
	return (
		<div className="flex items-center justify-end gap-2.5">
			<div aria-hidden className={cn("h-1 overflow-hidden rounded-full bg-surface-4", tier === "wide" ? "w-20" : "w-10")}>
				<div className="h-full rounded-full bg-info" style={{ width: `${Math.max(2, share)}%` }} />
			</div>
			<span className="num min-w-[3ch] font-medium text-fg">{integer(count)}</span>
		</div>
	);
}

function ScoreCell({ score, tier }: { score?: number; tier: Tier }) {
	if (score == null || !Number.isFinite(score)) {
		return (
			<span className="num text-fg-3">
				<span aria-hidden>—</span>
				<span className="sr-only">No score</span>
			</span>
		);
	}
	return (
		<div className="flex items-center justify-end gap-2">
			{tier === "wide" && <Meter value={score} tone={scoreTone(score)} className="w-14" label={`Score ${score} of 100`} />}
			<span className="num w-[3ch] text-right text-fg">{score}</span>
		</div>
	);
}

function MineCell({ mine, tier, now }: { mine?: MyAnalysis; tier: Tier; now: number }) {
	if (!mine) return <span className="sr-only">Not analyzed by you</span>;
	const s = mine.sentiment as Sentiment;
	const Icon = s === "bullish" ? ArrowUpRight : s === "bearish" ? ArrowDownRight : Minus;
	const full = `You: ${titleCase(s)} ${mine.score}`;
	const detail = `${full}. Analyzed ${mine.count === 1 ? "once" : `${mine.count} times`}, last ${ago(mine.at, now)}${
		mine.validated ? ", validated" : ""
	}`;
	return (
		<Badge tone={SENTIMENT_TONE[s] ?? "neutral"} icon={<Icon className="size-3" aria-hidden />} title={detail}>
			{tier === "wide" ? (
				<>
					You: {titleCase(s)} <span className="num">{mine.score}</span>
				</>
			) : (
				<>
					<span>You</span> <span className="num">{mine.score}</span>
					<span className="sr-only">{titleCase(s)}</span>
				</>
			)}
		</Badge>
	);
}

/** Hover / keyboard-focus reveal for per-row secondary controls. */
const REVEAL =
	"opacity-0 transition-opacity duration-fast focus-visible:opacity-100 group-focus-within:opacity-100 group-hover:opacity-100";

function CopyIcon({ mint, className }: { mint: string; className?: string }) {
	const [copied, setCopied] = useState(false);
	const timer = useRef<ReturnType<typeof setTimeout>>();
	useEffect(() => () => clearTimeout(timer.current), []);
	async function copy() {
		try {
			await navigator.clipboard.writeText(mint);
		} catch (err) {
			toast({ kind: "error", title: "Could not copy the mint address", description: errorMessage(err) });
			return;
		}
		setCopied(true);
		clearTimeout(timer.current);
		timer.current = setTimeout(() => setCopied(false), 1500);
		toast({ kind: "success", title: "Mint address copied", description: shortMint(mint, 6, 6), duration: 2500 });
	}
	return (
		<IconButton
			size="sm"
			label="Copy mint address"
			title={`Copy mint ${mint}`}
			onClick={copy}
			className={cn("size-6 text-fg-3", className)}
		>
			{copied ? <Check className="size-3.5 text-pos" aria-hidden /> : <Copy className="size-3.5" aria-hidden />}
		</IconButton>
	);
}

/* ---- Row ------------------------------------------------------------------- */

const Row = memo(function Row({
	entry,
	max,
	tier,
	mine,
	now,
	onAnalyze,
}: {
	entry: RankedEntry;
	max: number;
	tier: Tier;
	mine?: MyAnalysis;
	now: number;
	onAnalyze: (mint: string) => void;
}) {
	const sym = entry.symbol ? `$${entry.symbol}` : null;
	const label = sym ?? entry.name ?? shortMint(entry.mint);
	return (
		<tr className="group h-row transition-colors duration-fast focus-within:bg-surface-2 hover:bg-surface-2">
			<td className={cn("border-b border-line text-right", CELL)}>
				<RankCell rank={entry.rank} />
			</td>
			<td className={cn("border-b border-line", CELL)}>
				<div className="flex min-w-0 items-center gap-2.5">
					<TokenAvatar src={entry.image} symbol={entry.symbol} mint={entry.mint} size={28} />
					<div className="flex min-w-0 items-baseline gap-2">
						<span className="max-w-[10rem] shrink-0 truncate font-medium text-fg" title={sym ?? undefined}>
							{sym ?? <span className="text-fg-3">Unknown</span>}
						</span>
						{entry.name && (
							<span className="min-w-0 truncate text-fg-3" title={entry.name}>
								{entry.name}
							</span>
						)}
					</div>
					{tier === "wide" ? (
						<CopyMint mint={entry.mint} className={cn("h-6 shrink-0", REVEAL)} />
					) : (
						// Narrow: icon only, so the ticker and name keep their room on hover.
						<CopyIcon mint={entry.mint} className={cn("ml-auto", REVEAL)} />
					)}
				</div>
			</td>
			<td className={cn("border-b border-line text-right", CELL)}>
				<CountCell count={entry.count} max={max} tier={tier} />
			</td>
			<td className={cn("border-b border-line", CELL)}>
				{entry.lastSentiment ? (
					<SentimentBadge value={entry.lastSentiment} />
				) : (
					<span className="text-fg-3">
						<span aria-hidden>—</span>
						<span className="sr-only">No call yet</span>
					</span>
				)}
			</td>
			<td className={cn("border-b border-line text-right", CELL)}>
				<ScoreCell score={entry.lastScore} tier={tier} />
			</td>
			<td className={cn("border-b border-line", CELL)}>
				<MineCell mine={mine} tier={tier} now={now} />
			</td>
			<td className={cn("border-b border-line text-right", CELL)}>
				{tier === "wide" ? (
					<Button
						size="sm"
						variant="ghost"
						icon={<Zap className="size-3.5" aria-hidden />}
						onClick={() => onAnalyze(entry.mint)}
						aria-label={`Analyze ${label}`}
						className="text-fg-2 group-hover:bg-surface-3 group-hover:text-fg"
					>
						Analyze
					</Button>
				) : (
					<IconButton
						size="sm"
						label={`Analyze ${label}`}
						onClick={() => onAnalyze(entry.mint)}
						className="group-hover:bg-surface-3 group-hover:text-fg"
					>
						<Zap className="size-3.5" aria-hidden />
					</IconButton>
				)}
			</td>
		</tr>
	);
});

/* ---- Table ----------------------------------------------------------------- */

export function RankTable({
	entries,
	max,
	tier,
	sort,
	onSort,
	analyzed,
	now,
	onAnalyze,
}: {
	entries: RankedEntry[];
	max: number;
	tier: Tier;
	sort: Sort;
	onSort: (k: SortKey) => void;
	analyzed: Record<string, MyAnalysis>;
	now: number;
	onAnalyze: (mint: string) => void;
}) {
	return (
		<table className="w-full table-fixed border-separate border-spacing-0 text-base">
			<caption className="sr-only">Most analyzed tokens, ranked by number of analyses across all agents</caption>
			<Cols tier={tier} />
			<Head sort={sort} onSort={onSort} />
			<tbody>
				{entries.map((e) => (
					<Row
						key={e.mint}
						entry={e}
						max={max}
						tier={tier}
						mine={analyzed[e.mint]}
						now={now}
						onAnalyze={onAnalyze}
					/>
				))}
			</tbody>
		</table>
	);
}

export function RankTableSkeleton({ tier, rows = 12 }: { tier: Tier; rows?: number }) {
	return (
		<table className="w-full table-fixed border-separate border-spacing-0 text-base" aria-hidden>
			<Cols tier={tier} />
			<thead>
				<tr>
					{["Rank", "Token", "Analyses", "Last call", "Last score", "You", ""].map((h, i) => (
						<th
							key={i}
							scope="col"
							className={cn(
								"sticky top-0 z-10 h-9 border-b border-line bg-surface-1 text-caption font-medium uppercase text-fg-3",
								CELL,
								i === 0 || i === 2 || i === 4 ? "text-right" : "text-left",
							)}
						>
							{h}
						</th>
					))}
				</tr>
			</thead>
			<tbody>
				{Array.from({ length: rows }).map((_, i) => (
					<tr key={i} className="h-row">
						<td className={cn("border-b border-line", CELL)}>
							<Skeleton className="ml-auto h-3 w-5" />
						</td>
						<td className={cn("border-b border-line", CELL)}>
							<div className="flex items-center gap-2.5">
								<Skeleton className="size-7 shrink-0 rounded-full" />
								<Skeleton className="h-3 w-14" />
								<div aria-hidden className="skeleton skeleton-shimmer h-3" style={{ width: 60 + ((i * 37) % 80) }} />
							</div>
						</td>
						<td className={cn("border-b border-line", CELL)}>
							<div className="flex items-center justify-end gap-2.5">
								<Skeleton className={cn("h-1 rounded-full", tier === "wide" ? "w-20" : "w-10")} />
								<Skeleton className="h-3 w-8" />
							</div>
						</td>
						<td className={cn("border-b border-line", CELL)}>
							<Skeleton className="h-5 w-16 rounded" />
						</td>
						<td className={cn("border-b border-line", CELL)}>
							<Skeleton className="ml-auto h-3 w-12" />
						</td>
						<td className={cn("border-b border-line", CELL)} />
						<td className={cn("border-b border-line", CELL)}>
							<Skeleton className={cn("ml-auto h-6 rounded", tier === "wide" ? "w-16" : "w-6")} />
						</td>
					</tr>
				))}
			</tbody>
		</table>
	);
}
