import { useId, type ReactNode } from "react";
import { ArrowDownRight, ArrowUpRight, Check, Minus, Plus, Sparkles } from "lucide-react";
import { MAX_RISK_FACTORS, RISK_FACTORS, type RiskFactorInfo } from "../../../shared/analysis";
import type {
	HolderConcentration,
	LiquidityDepth,
	RiskFactor,
	RiskLevel,
	Sentiment,
	TrendDirection,
	VolumeProfile,
} from "../../../shared/types";
import { cn } from "../../lib/cn";
import { RISK_TONE, SENTIMENT_TONE } from "../token/widgets";
import { FieldMessage, Segmented, type SegmentOption } from "../ui/primitives";

/* ---- Options ---------------------------------------------------------- */

export const SENTIMENT_OPTIONS: readonly SegmentOption<Sentiment>[] = [
	{ value: "bullish", label: <><ArrowUpRight className="size-3.5" aria-hidden />Bullish</>, tone: SENTIMENT_TONE.bullish },
	{ value: "neutral", label: <><Minus className="size-3.5" aria-hidden />Neutral</>, tone: SENTIMENT_TONE.neutral },
	{ value: "bearish", label: <><ArrowDownRight className="size-3.5" aria-hidden />Bearish</>, tone: SENTIMENT_TONE.bearish },
];

const dot = (cls: string) => <span aria-hidden className={cn("size-1.5 rounded-full", cls)} />;

export const RISK_OPTIONS: readonly SegmentOption<RiskLevel>[] = [
	{ value: "low", label: <>{dot("bg-pos")}Low</>, tone: RISK_TONE.low },
	{ value: "medium", label: <>{dot("bg-warn")}Medium</>, tone: RISK_TONE.medium },
	{ value: "high", label: <>{dot("bg-orange")}High</>, tone: RISK_TONE.high },
	{ value: "critical", label: <>{dot("bg-neg")}Critical</>, tone: RISK_TONE.critical },
];

export const TREND_OPTIONS: readonly SegmentOption<TrendDirection>[] = [
	{ value: "up", label: "Up", title: "Price trending up" },
	{ value: "sideways", label: "Sideways", title: "No clear direction" },
	{ value: "down", label: "Down", title: "Price trending down" },
	{ value: "reversal", label: "Reversal", title: "Short-term move against the 24h trend" },
];

export const VOLUME_OPTIONS: readonly SegmentOption<VolumeProfile>[] = [
	{ value: "surging", label: "Surging", title: "24h volume above 50% of market cap" },
	{ value: "rising", label: "Rising", title: "24h volume 15-50% of market cap" },
	{ value: "stable", label: "Stable", title: "24h volume 3-15% of market cap" },
	{ value: "declining", label: "Declining", title: "24h volume 0.5-3% of market cap" },
	{ value: "dead", label: "Dead", title: "Almost no volume" },
];

export const LIQUIDITY_OPTIONS: readonly SegmentOption<LiquidityDepth>[] = [
	{ value: "deep", label: "Deep", title: "$1M or more" },
	{ value: "moderate", label: "Moderate", title: "$250K to $1M" },
	{ value: "shallow", label: "Shallow", title: "$50K to $250K" },
	{ value: "dry", label: "Dry", title: "Under $50K" },
];

export const HOLDER_OPTIONS: readonly SegmentOption<HolderConcentration>[] = [
	{ value: "distributed", label: "Distributed", title: "Top 10 hold 20% or less" },
	{ value: "moderate", label: "Moderate", title: "Top 10 hold 20-50%" },
	{ value: "concentrated", label: "Concentrated", title: "Top 10 hold 50-80%" },
	{ value: "whale_dominated", label: "Whales", title: "Whale dominated: top 10 hold more than 80%" },
];

export function sentenceCase(s: string): string {
	const t = s.replace(/_/g, " ");
	return t.charAt(0).toUpperCase() + t.slice(1);
}

/* ---- AI marker -------------------------------------------------------- */

export function AiMarker({ show }: { show: boolean }) {
	if (!show) return null;
	return (
		<span
			title="Set by AI auto-fill. Disappears when you edit the field."
			className="inline-flex h-4 items-center gap-0.5 rounded-sm bg-violet/10 px-1 text-[11px] font-medium leading-none text-violet"
		>
			<Sparkles className="size-2.5" aria-hidden />
			AI
			<span className="sr-only"> filled</span>
		</span>
	);
}

/* ---- Field header ----------------------------------------------------- */

function Header({
	label,
	labelId,
	htmlFor,
	ai,
	extra,
	right,
}: {
	label: string;
	labelId?: string;
	htmlFor?: string;
	ai?: boolean;
	extra?: ReactNode;
	right?: ReactNode;
}) {
	return (
		<div className="mb-1.5 flex min-h-5 items-center justify-between gap-2">
			<div className="flex min-w-0 items-center gap-1.5">
				{htmlFor ? (
					<label id={labelId} htmlFor={htmlFor} className="truncate text-sm font-medium text-fg-2">
						{label}
					</label>
				) : (
					<span id={labelId} className="truncate text-sm font-medium text-fg-2">
						{label}
					</span>
				)}
				{extra}
				<AiMarker show={!!ai} />
			</div>
			{right && <div className="shrink-0 text-sm text-fg-3">{right}</div>}
		</div>
	);
}

/** "Data says 64" link that restores the computed value. */
function Reference({ value, current, onUse }: { value?: number | string; current: number | string; onUse: () => void }) {
	if (value == null) return null;
	if (value === current) return null;
	return (
		<button
			type="button"
			onClick={onUse}
			title="Use the value computed from the snapshot"
			className="num rounded text-fg-3 underline decoration-dotted underline-offset-2 hover:text-fg"
		>
			Data says {value}
		</button>
	);
}

/* ---- Choice (segmented) ---------------------------------------------- */

export function ChoiceField<T extends string>({
	label,
	value,
	options,
	onChange,
	ai,
	reference,
	size = "md",
}: {
	label: string;
	value: T;
	options: readonly SegmentOption<T>[];
	onChange: (v: T) => void;
	ai?: boolean;
	/** Value computed from data; shown as a restorable hint when different. */
	reference?: T;
	size?: "sm" | "md";
}) {
	const refLabel = reference != null ? options.find((o) => o.value === reference) : undefined;
	const refText = refLabel
		? typeof refLabel.label === "string"
			? refLabel.label
			: sentenceCase(String(refLabel.value))
		: undefined;
	return (
		<div className="min-w-0">
			<Header
				label={label}
				ai={ai}
				right={
					reference != null && reference !== value ? (
						<button
							type="button"
							onClick={() => onChange(reference)}
							title="Use the value computed from the snapshot"
							className="rounded text-fg-3 underline decoration-dotted underline-offset-2 hover:text-fg"
						>
							Data says {refText}
						</button>
					) : null
				}
			/>
			<Segmented label={label} value={value} options={options} onChange={onChange} size={size} full />
		</div>
	);
}

/* ---- Scale (slider + number) ----------------------------------------- */

export function ScaleField({
	label,
	value,
	onChange,
	ai,
	reference,
	valueText,
	fill,
	error,
	legend,
	reading,
}: {
	label: string;
	value: number;
	onChange: (v: number) => void;
	ai?: boolean;
	reference?: number;
	valueText?: string;
	fill?: string;
	error?: string;
	/** Labels under the track: [left, middle, right]. */
	legend?: [string, string, string];
	/** Short reading of the value, shown next to the label. */
	reading?: ReactNode;
}) {
	const id = useId();
	const clampN = (n: number) => Math.max(0, Math.min(100, Math.round(n)));
	return (
		<div className="min-w-0">
			<Header
				label={label}
				htmlFor={id}
				ai={ai}
				extra={reading}
				right={<Reference value={reference} current={value} onUse={() => reference != null && onChange(reference)} />}
			/>
			<div className="flex items-center gap-3">
				<div className="min-w-0 flex-1">
					<input
						id={id}
						type="range"
						min={0}
						max={100}
						step={1}
						value={value}
						aria-valuetext={valueText ?? String(value)}
						onChange={(e) => onChange(clampN(Number(e.target.value)))}
						className="range block"
						style={{ ["--range-pct" as string]: `${value}%`, ...(fill ? { ["--range-fill" as string]: fill } : {}) }}
					/>
				</div>
				<input
					type="number"
					inputMode="numeric"
					aria-label={`${label} value`}
					min={0}
					max={100}
					value={Number.isFinite(value) ? value : ""}
					onChange={(e) => {
						if (e.target.value === "") return;
						onChange(clampN(Number(e.target.value)));
					}}
					aria-invalid={!!error || undefined}
					className="field num h-8 w-14 shrink-0 px-2 text-center text-base"
				/>
			</div>
			{legend && (
				<div aria-hidden className="mr-[68px] mt-0.5 flex items-center justify-between gap-2 text-sm text-fg-3">
					<span>{legend[0]}</span>
					<span>{legend[1]}</span>
					<span>{legend[2]}</span>
				</div>
			)}
			{error && <FieldMessage error>{error}</FieldMessage>}
		</div>
	);
}

/* ---- Risk factors ----------------------------------------------------- */

const NEGATIVE_FACTORS = RISK_FACTORS.filter((f) => f.kind === "negative");
const POSITIVE_FACTORS = RISK_FACTORS.filter((f) => f.kind === "positive");

export function RiskFactorPicker({
	value,
	onChange,
	ai,
	suggested,
	error,
}: {
	value: readonly RiskFactor[];
	onChange: (v: RiskFactor[]) => void;
	ai?: boolean;
	/** Factors the data rules picked, marked with a dot. */
	suggested: readonly RiskFactor[];
	error?: string;
}) {
	const full = value.length >= MAX_RISK_FACTORS;
	const toggle = (id: RiskFactor) =>
		onChange(value.includes(id) ? value.filter((x) => x !== id) : full ? [...value] : [...value, id]);
	const counterId = useId();

	const group = (title: string, items: readonly RiskFactorInfo[], kind: "negative" | "positive") => (
		<div role="group" aria-label={title} aria-describedby={counterId}>
			<div className="mb-1.5 flex items-center gap-1.5 text-sm text-fg-3">
				{kind === "negative" ? (
					<Minus className="size-3.5 text-neg" aria-hidden />
				) : (
					<Plus className="size-3.5 text-pos" aria-hidden />
				)}
				{title}
			</div>
			<div className="flex flex-wrap gap-1.5">
				{items.map((f) => {
					const on = value.includes(f.id);
					const disabled = !on && full;
					const isSuggested = suggested.includes(f.id);
					return (
						<button
							key={f.id}
							type="button"
							aria-pressed={on}
							disabled={disabled}
							title={`${f.hint}${isSuggested ? ". Picked by the data rules." : ""}${disabled ? ". Limit reached." : ""}`}
							onClick={() => toggle(f.id)}
							className={cn(
								"inline-flex h-7 items-center gap-1 rounded-md border px-2 text-sm transition-colors duration-fast",
								"disabled:cursor-not-allowed disabled:opacity-40",
								on
									? kind === "negative"
										? "border-neg/30 bg-neg/10 font-medium text-neg"
										: "border-pos/30 bg-pos/10 font-medium text-pos"
									: "border-line-strong bg-surface-1 text-fg-2 enabled:hover:border-line-control enabled:hover:text-fg",
							)}
						>
							{on && <Check className="-ml-0.5 size-3.5" aria-hidden />}
							{f.label}
							{isSuggested && !on && (
								<span aria-hidden className="ml-0.5 size-1 rounded-full bg-fg-3" />
							)}
						</button>
					);
				})}
			</div>
		</div>
	);

	return (
		<div>
			<Header
				label="Risk factors"
				ai={ai}
				right={
					<span id={counterId} className={cn("num", full ? "text-warn" : error ? "text-neg" : "text-fg-3")}>
						{value.length} of {MAX_RISK_FACTORS} selected
					</span>
				}
			/>
			<div className="space-y-3 rounded-md border border-line bg-surface-1 p-3">
				{group("Warning signs", NEGATIVE_FACTORS, "negative")}
				{group("Positive signs", POSITIVE_FACTORS, "positive")}
			</div>
			<div className="mt-1.5 flex items-center justify-between gap-3 text-sm">
				{error ? (
					<p role="alert" className="text-neg">
						{error}
					</p>
				) : full ? (
					<p className="text-warn">Limit of {MAX_RISK_FACTORS} reached. Deselect one to pick another.</p>
				) : (
					<p className="text-fg-3">Pick 1 to {MAX_RISK_FACTORS}. Hover a factor to see what it means.</p>
				)}
				<span className="inline-flex shrink-0 items-center gap-1.5 text-fg-3">
					<span aria-hidden className="size-1 rounded-full bg-fg-3" />
					Suggested by data
				</span>
			</div>
		</div>
	);
}
