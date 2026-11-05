import type { ReactNode } from "react";
import { ArrowRight, Database, Flag, Layers, Sparkles, Target } from "lucide-react";
import type { AnalysisStats } from "../../../shared/types";
import { cn } from "../../lib/cn";
import { integer, pct } from "../../lib/format";
import { useApp } from "../../lib/store";
import { Meter } from "../token/widgets";
import { Card, Skeleton } from "../ui/primitives";

/*
 * One row at every width. Wide: the progress and session tiles get a bit more
 * room. Compact (container under ~1040px): only progress is wider, no icons,
 * shorter secondary lines, and the progress meter moves next to the percentage.
 */
const gridClass = (compact: boolean) =>
	cn(
		"grid gap-3",
		compact
			? "grid-cols-[repeat(3,minmax(0,1fr))_minmax(0,1.35fr)_minmax(0,1fr)]"
			: "grid-cols-[repeat(3,minmax(0,1fr))_minmax(0,1.45fr)_minmax(0,1.45fr)]",
	);

function Tile({
	label,
	icon,
	children,
	compact,
}: {
	label: string;
	icon: ReactNode;
	children: ReactNode;
	compact: boolean;
}) {
	return (
		<Card className="flex min-w-0 flex-col px-4 py-3.5">
			<div className="flex h-4 items-center justify-between gap-2">
				<h2 className="truncate text-caption uppercase text-fg-3" title={label}>
					{label}
				</h2>
				{!compact && (
					<span className="shrink-0 text-fg-3" aria-hidden>
						{icon}
					</span>
				)}
			</div>
			{children}
		</Card>
	);
}

function Value({ children, className }: { children: ReactNode; className?: string }) {
	return <div className={cn("num mt-1.5 truncate text-kpi font-semibold text-fg", className)}>{children}</div>;
}

function Sub({ children, className, title }: { children: ReactNode; className?: string; title?: string }) {
	return (
		<div className={cn("num mt-0.5 truncate text-sm text-fg-3", className)} title={title}>
			{children}
		</div>
	);
}

export function KpiRow({ stats, compact = false }: { stats: AnalysisStats; compact?: boolean }) {
	const { totalAnalyses, uniqueTokens, goal, progress } = stats;
	const perToken = uniqueTokens > 0 ? (totalAnalyses / uniqueTokens).toFixed(1) : null;
	const remaining = Math.max(0, goal - totalAnalyses);
	const reached = goal > 0 && totalAnalyses >= goal;
	const ofGoal = `${integer(totalAnalyses)} of ${integer(goal)}`;
	const meter = (
		<Meter
			value={progress}
			tone={reached ? "pos" : "brand"}
			label="Progress to dataset goal"
			className={compact ? "min-w-0 flex-1" : "mt-2.5"}
		/>
	);

	return (
		<section aria-label="Dataset totals" className={gridClass(compact)}>
			<Tile label="Total analyses" icon={<Database className="size-3.5" />} compact={compact}>
				<Value>{integer(totalAnalyses)}</Value>
				<Sub>Across all agents</Sub>
			</Tile>
			<Tile label="Unique tokens" icon={<Layers className="size-3.5" />} compact={compact}>
				<Value>{integer(uniqueTokens)}</Value>
				<Sub title={perToken ? `${perToken} analyses per token on average` : undefined}>
					{perToken ? (compact ? `${perToken} per token` : `${perToken} analyses per token`) : "No tokens yet"}
				</Sub>
			</Tile>
			<Tile label="Dataset goal" icon={<Flag className="size-3.5" />} compact={compact}>
				<Value>{integer(goal)}</Value>
				<Sub>{reached ? "Goal reached" : `${integer(remaining)} to go`}</Sub>
			</Tile>
			<Tile label="Progress to goal" icon={<Target className="size-3.5" />} compact={compact}>
				<div className="mt-1.5 flex h-8 min-w-0 items-center justify-between gap-3">
					<span className={cn("num text-kpi font-semibold", reached ? "text-pos" : "text-fg")}>{pct(progress)}</span>
					{compact ? (
						meter
					) : (
						<span className="num truncate text-sm text-fg-3" title={`${ofGoal} analyses`}>
							{ofGoal}
						</span>
					)}
				</div>
				{compact ? <Sub title={`${ofGoal} analyses`}>{ofGoal}</Sub> : meter}
			</Tile>
			<SessionTile compact={compact} />
		</section>
	);
}

function SessionTile({ compact }: { compact: boolean }) {
	const session = useApp((s) => s.xp.session);
	const submissions = useApp((s) => s.xp.submissions);
	const validated = useApp((s) => s.xp.validated);
	const setView = useApp((s) => s.setView);
	const rate = submissions > 0 ? Math.round((validated / submissions) * 100) : null;
	const full = `${integer(submissions)} ${submissions === 1 ? "submission" : "submissions"} · ${integer(validated)} validated${
		rate != null ? ` (${rate}%)` : ""
	}`;

	return (
		<Tile label="Your session" icon={<Sparkles className="size-3.5 text-brand" />} compact={compact}>
			<div className="mt-1.5 flex h-8 min-w-0 items-baseline gap-1.5">
				<span className={cn("num text-kpi font-semibold", session > 0 ? "text-fg" : "text-fg-3")}>{integer(session)}</span>
				<span className="text-base font-medium text-fg-3">XP</span>
			</div>
			{submissions > 0 ? (
				<Sub title={full}>{compact ? `${integer(validated)} of ${integer(submissions)} validated` : full}</Sub>
			) : (
				<div className="mt-0.5 flex min-w-0 items-center gap-1 text-sm text-fg-3">
					<button
						type="button"
						onClick={() => setView("analyze")}
						className="inline-flex shrink-0 items-center gap-1 rounded font-medium text-brand underline-offset-2 hover:underline"
					>
						Analyze a token
						<ArrowRight className="size-3" aria-hidden />
					</button>
					{!compact && <span className="truncate">to start earning</span>}
				</div>
			)}
		</Tile>
	);
}

const Line = ({ h = "h-4", w, className }: { h?: string; w: string; className?: string }) => (
	<div className={cn("flex items-center", h, className)}>
		<Skeleton className={cn(w, h === "h-8" ? "h-6" : "h-3")} />
	</div>
);

/** Same box model as the real tiles, so nothing jumps when data arrives. */
export function KpiRowSkeleton({ compact = false }: { compact?: boolean }) {
	const widths = ["w-20", "w-16", "w-20", "w-16", "w-12"];
	return (
		<div className={gridClass(compact)} aria-hidden>
			{widths.map((w, i) => (
				<Card key={i} className="px-4 py-3.5">
					<Line w={i === 3 ? "w-28" : "w-24"} />
					<div className="mt-1.5 flex h-8 items-center justify-between gap-3">
						<Skeleton className={cn("h-6", w)} />
						{i === 3 && <Skeleton className={cn("h-1.5 rounded-full", compact ? "flex-1" : "w-24")} />}
					</div>
					{i === 3 && !compact ? (
						<Skeleton className="mt-2.5 h-1.5 w-full rounded-full" />
					) : (
						<Line w={i === 4 ? "w-32" : "w-24"} className="mt-0.5 max-w-full" />
					)}
				</Card>
			))}
		</div>
	);
}
