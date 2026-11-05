import { useId, type ReactNode } from "react";
import { FileText } from "lucide-react";
import { SUMMARY_MAX, SUMMARY_MIN, type FieldErrors } from "../../../shared/analysis";
import type { AnalysisFields } from "../../../shared/types";
import { cn } from "../../lib/cn";
import { integer } from "../../lib/format";
import { Button } from "../ui/Button";
import { Card, SectionTitle, TextArea } from "../ui/primitives";
import type { FieldKey } from "./drafts";
import {
	AiMarker,
	ChoiceField,
	HOLDER_OPTIONS,
	LIQUIDITY_OPTIONS,
	RISK_OPTIONS,
	RiskFactorPicker,
	SENTIMENT_OPTIONS,
	ScaleField,
	TREND_OPTIONS,
	VOLUME_OPTIONS,
} from "./fields";
import { useWidth } from "./hooks";

/** Section that holds each field, for "Fix 1 issue" jumps. */
export const FIELD_SECTION: Record<FieldKey, string> = {
	sentiment: "verdict",
	score: "verdict",
	riskLevel: "risk",
	riskFactors: "risk",
	buyPressure: "market",
	volatilityScore: "market",
	trendDirection: "market",
	volumeProfile: "market",
	liquidityDepth: "market",
	holderConcentration: "market",
	summary: "summary",
};

export const sectionId = (mint: string, section: string) => `analyze-${section}-${mint.slice(0, 8)}`;
export const summaryId = (mint: string) => `analyze-summary-input-${mint.slice(0, 8)}`;

export function scoreTone(score: number): { color: string; word: string } {
	if (score < 20) return { color: "rgb(var(--neg))", word: "strongly bearish" };
	if (score < 35) return { color: "rgb(var(--neg))", word: "leaning bearish" };
	if (score <= 65) return { color: "rgb(var(--warn))", word: "neutral" };
	if (score <= 80) return { color: "rgb(var(--pos))", word: "leaning bullish" };
	return { color: "rgb(var(--pos))", word: "strongly bullish" };
}

function Section({
	id,
	step,
	title,
	description,
	action,
	children,
}: {
	id: string;
	step: number;
	title: string;
	description?: string;
	action?: ReactNode;
	children: ReactNode;
}) {
	return (
		<Card as="section" className="scroll-mt-4 p-5">
			<div id={id} tabIndex={-1} className="rounded focus:outline-none">
				<SectionTitle action={action} className="mb-1">
					<span className="num mr-2 text-fg-disabled">{step}</span>
					{title}
				</SectionTitle>
				{description && <p className="mb-4 text-sm text-fg-3">{description}</p>}
			</div>
			{children}
		</Card>
	);
}

export function AnalysisForm({
	mint,
	fields,
	defaults,
	aiFields,
	errors,
	showErrors,
	onChange,
	onInsertSummary,
}: {
	mint: string;
	fields: AnalysisFields;
	defaults: AnalysisFields;
	aiFields: readonly FieldKey[];
	errors: FieldErrors;
	/** Show validation messages inline (after a submit attempt or once edited). */
	showErrors: boolean;
	onChange: <K extends FieldKey>(key: K, value: AnalysisFields[K]) => void;
	onInsertSummary: () => void;
}) {
	const [ref, width] = useWidth<HTMLDivElement>();
	const twoCol = width >= 620;
	const ai = (k: FieldKey) => aiFields.includes(k);
	const summaryLen = fields.summary.trim().length;
	const summaryHintId = useId();
	const score = scoreTone(fields.score);
	const grid = cn("grid gap-x-6 gap-y-5", twoCol ? "grid-cols-2" : "grid-cols-1");

	return (
		<div ref={ref} className="space-y-4">
			<Section
				id={sectionId(mint, "verdict")}
				step={1}
				title="Verdict"
				description="Your overall call on this token and how strongly you hold it."
			>
				<div className={grid}>
					<ChoiceField
						label="Sentiment"
						value={fields.sentiment}
						options={SENTIMENT_OPTIONS}
						onChange={(v) => onChange("sentiment", v)}
						ai={ai("sentiment")}
						reference={defaults.sentiment}
					/>
					<ScaleField
						label="Score"
						value={fields.score}
						onChange={(v) => onChange("score", v)}
						ai={ai("score")}
						reference={defaults.score}
						fill={score.color}
						valueText={`${fields.score}, ${score.word}`}
						legend={["Bearish", "Neutral", "Bullish"]}
						reading={
							<span className="truncate text-sm" style={{ color: score.color }}>
								{score.word.charAt(0).toUpperCase() + score.word.slice(1)}
							</span>
						}
						error={showErrors ? errors.score : undefined}
					/>
				</div>
			</Section>

			<Section
				id={sectionId(mint, "risk")}
				step={2}
				title="Risk"
				description="How likely holders are to lose money, and the signs that point to it."
			>
				<div className="space-y-5">
					<ChoiceField
						label="Risk level"
						value={fields.riskLevel}
						options={RISK_OPTIONS}
						onChange={(v) => onChange("riskLevel", v)}
						ai={ai("riskLevel")}
						reference={defaults.riskLevel}
					/>
					<RiskFactorPicker
						value={fields.riskFactors}
						onChange={(v) => onChange("riskFactors", v)}
						ai={ai("riskFactors")}
						suggested={defaults.riskFactors}
						error={showErrors || fields.riskFactors.length === 0 ? errors.riskFactors : undefined}
					/>
				</div>
			</Section>

			<Section
				id={sectionId(mint, "market")}
				step={3}
				title="Market structure"
				description="Read the trading activity. Values computed from the snapshot are shown as Data says."
			>
				<div className={grid}>
					<ScaleField
						label="Buy pressure"
						value={fields.buyPressure}
						onChange={(v) => onChange("buyPressure", v)}
						ai={ai("buyPressure")}
						reference={defaults.buyPressure}
						legend={["Sellers", "Balanced", "Buyers"]}
						valueText={`${fields.buyPressure} of 100`}
						error={showErrors ? errors.buyPressure : undefined}
					/>
					<ScaleField
						label="Volatility"
						value={fields.volatilityScore}
						onChange={(v) => onChange("volatilityScore", v)}
						ai={ai("volatilityScore")}
						reference={defaults.volatilityScore}
						fill="rgb(var(--info))"
						legend={["Calm", "Active", "Wild"]}
						valueText={`${fields.volatilityScore} of 100`}
						error={showErrors ? errors.volatilityScore : undefined}
					/>
					<ChoiceField
						label="Trend"
						value={fields.trendDirection}
						options={TREND_OPTIONS}
						onChange={(v) => onChange("trendDirection", v)}
						ai={ai("trendDirection")}
						reference={defaults.trendDirection}
						size="sm"
					/>
					<ChoiceField
						label="Volume profile"
						value={fields.volumeProfile}
						options={VOLUME_OPTIONS}
						onChange={(v) => onChange("volumeProfile", v)}
						ai={ai("volumeProfile")}
						reference={defaults.volumeProfile}
						size="sm"
					/>
					<ChoiceField
						label="Liquidity depth"
						value={fields.liquidityDepth}
						options={LIQUIDITY_OPTIONS}
						onChange={(v) => onChange("liquidityDepth", v)}
						ai={ai("liquidityDepth")}
						reference={defaults.liquidityDepth}
						size="sm"
					/>
					<ChoiceField
						label="Holder concentration"
						value={fields.holderConcentration}
						options={HOLDER_OPTIONS}
						onChange={(v) => onChange("holderConcentration", v)}
						ai={ai("holderConcentration")}
						reference={defaults.holderConcentration}
						size="sm"
					/>
				</div>
			</Section>

			<Section
				id={sectionId(mint, "summary")}
				step={4}
				title="Summary"
				description="Your thesis in plain words. Mention the numbers that drove your call."
				action={
					<div className="-my-1 flex items-center gap-2">
						<AiMarker show={ai("summary")} />
						<Button variant="ghost" size="sm" icon={<FileText className="size-3.5" />} onClick={onInsertSummary}>
							Insert data summary
						</Button>
					</div>
				}
			>
				<label htmlFor={summaryId(mint)} className="sr-only">
					Summary
				</label>
				<TextArea
					id={summaryId(mint)}
					rows={5}
					value={fields.summary}
					maxLength={SUMMARY_MAX}
					onChange={(e) => onChange("summary", e.target.value)}
					placeholder="Example: Buyers outnumber sellers 2 to 1 and liquidity is shallow at $42K. Top 10 hold 61%, so one whale can move the price."
					invalid={showErrors && !!errors.summary}
					aria-describedby={summaryHintId}
					spellCheck
				/>
				<div id={summaryHintId} className="mt-1.5 flex items-center justify-between gap-3 text-sm">
					{showErrors && errors.summary ? (
						<span role="alert" className="text-neg">
							{errors.summary}
						</span>
					) : (
						<span className={summaryLen >= SUMMARY_MIN ? "text-fg-3" : "text-fg-2"}>
							At least {SUMMARY_MIN} characters
							{summaryLen < SUMMARY_MIN && summaryLen > 0 && (
								<span className="num text-fg-3"> · {SUMMARY_MIN - summaryLen} to go</span>
							)}
						</span>
					)}
					<span className={cn("num shrink-0", summaryLen > SUMMARY_MAX * 0.95 ? "text-warn" : "text-fg-3")}>
						{integer(fields.summary.length)} / {integer(SUMMARY_MAX)}
					</span>
				</div>
			</Section>
		</div>
	);
}
