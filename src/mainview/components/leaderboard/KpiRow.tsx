import type { ReactNode } from "react";
import { ArrowRight, Database, Flag, Layers, Sparkles, Target } from "lucide-react";
import type { AnalysisStats } from "../../../shared/types";
import { cn } from "../../lib/cn";
import { integer, pct } from "../../lib/format";
import { useApp } from "../../lib/store";
import { Meter } from "../token/widgets";
import { Card, Skeleton } from "../ui/primitives";

/*
 * Narrow windows: two rows (three small tiles, then progress + session).
 * From xl: one row, with the two wide tiles taking a bit more space.
 */
const GRID =
	"grid grid-cols-6 gap-3 xl:grid-cols-[repeat(3,minmax(0,1fr))_minmax(0,1.45fr)_minmax(0,1.45fr)]";
const SMALL = "col-span-2 xl:col-span-1";
const WIDE = "col-span-3 xl:col-span-1";

function Tile({
	label,
	icon,
	children,
	className,
	title,
}: {
	label: string;
	icon: ReactNode;
	children: ReactNode;
	className?: string;
	title?: string;
}) {
	return (
		<Card className={cn("flex min-w-0 flex-col px-4 py-3.5", className)}>
			<div className="flex items-center justify-between gap-2" title={title}>
				<h2 className="truncate text-caption uppercase text-fg-3">{label}</h2>
				<span className="shrink-0 text-fg-3" aria-hidden>
					{icon}
				</span>
			</div>
			{children}
		</Card>
	);
}

function Value({ children, className }: { children: ReactNode; className?: string }) {
	return <div className={cn("num mt-1.5 truncate text-kpi font-semibold text-fg", className)}>{children}</div>;
}

function Sub({ children, className }: { children: ReactNode; className?: string }) {
	return <div className={cn("num mt-0.5 truncate text-sm text-fg-3", className)}>{children}</div>;
}

export function KpiRow({ stats }: { stats: AnalysisStats }) {
	const { totalAnalyses, uniqueTokens, goal, progress } = stats;
	const perToken = uniqueTokens > 0 ? totalAnalyses / uniqueTokens : null;
	const remaining = Math.max(0, goal - totalAnalyses);
	const reached = goal > 0 && totalAnalyses >= goal;

	return (
		<section aria-label="Dataset totals" className={GRID}>
			<Tile label="Total analyses" icon={<Database className="size-3.5" />} className={SMALL}>
				<Value>{integer(totalAnalyses)}</Value>
				<Sub>Across all agents</Sub>
			</Tile>
			<Tile label="Unique tokens" icon={<Layers className="size-3.5" />} className={SMALL}>
				<Value>{integer(uniqueTokens)}</Value>
				<Sub>{perToken != null ? `${perToken.toFixed(1)} analyses per token` : "No tokens yet"}</Sub>
			</Tile>
			<Tile label="Dataset goal" icon={<Flag className="size-3.5" />} className={SMALL}>
				<Value>{integer(goal)}</Value>
				<Sub>{reached ? "Goal reached" : `${integer(remaining)} to go`}</Sub>
			</Tile>
			<Tile label="Progress to goal" icon={<Target className="size-3.5" />} className={WIDE}>
				<div className="mt-1.5 flex min-w-0 items-baseline justify-between gap-3">
					<span className={cn("num text-kpi font-semibold", reached ? "text-pos" : "text-fg")}>{pct(progress)}</span>
					<span className="num truncate text-sm text-fg-3" title={`${integer(totalAnalyses)} of ${integer(goal)} analyses`}>
						{integer(totalAnalyses)} of {integer(goal)}
					</span>
				</div>
				<Meter value={progress} tone={reached ? "pos" : "brand"} label="Progress to dataset goal" className="mt-2.5" />
			</Tile>
			<SessionTile className={WIDE} />
		</section>
	);
}

function SessionTile({ className }: { className?: string }) {
	const session = useApp((s) => s.xp.session);
	const submissions = useApp((s) => s.xp.submissions);
	const validated = useApp((s) => s.xp.validated);
	const setView = useApp((s) => s.setView);
	const rate = submissions > 0 ? Math.round((validated / submissions) * 100) : null;

	return (
		<Tile label="Your session" icon={<Sparkles className="size-3.5 text-brand" />} className={className}>
			<div className="mt-1.5 flex min-w-0 items-baseline gap-1.5">
				<span className={cn("num text-kpi font-semibold", session > 0 ? "text-fg" : "text-fg-3")}>{integer(session)}</span>
				<span className="text-base font-medium text-fg-3">XP</span>
			</div>
			{submissions > 0 ? (
				<Sub>
					{integer(submissions)} {submissions === 1 ? "submission" : "submissions"} · {integer(validated)} validated
					{rate != null && ` (${rate}%)`}
				</Sub>
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
					<span className="truncate">to start earning</span>
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
export function KpiRowSkeleton() {
	return (
		<div className={GRID} aria-hidden>
			{["w-24", "w-20", "w-20"].map((w, i) => (
				<Card key={i} className={cn("px-4 py-3.5", SMALL)}>
					<Line w={w} />
					<Line h="h-8" w="w-20" className="mt-1.5" />
					<Line w="w-28" className="mt-0.5" />
				</Card>
			))}
			<Card className={cn("px-4 py-3.5", WIDE)}>
				<Line w="w-28" />
				<div className="mt-1.5 flex h-8 items-center justify-between">
					<Skeleton className="h-6 w-16" />
					<Skeleton className="h-3 w-24" />
				</div>
				<Skeleton className="mt-2.5 h-1.5 w-full rounded-full" />
			</Card>
			<Card className={cn("px-4 py-3.5", WIDE)}>
				<Line w="w-24" />
				<Line h="h-8" w="w-16" className="mt-1.5" />
				<Line w="w-40" className="mt-0.5" />
			</Card>
		</div>
	);
}
