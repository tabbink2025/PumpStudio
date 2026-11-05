import { useEffect, useMemo } from "react";
import { AlertTriangle, ArrowLeft, ArrowRight, CheckCircle2, RotateCcw, Sparkles } from "lucide-react";
import { VALIDATION_DEVIATION_PCT } from "../../../shared/analysis";
import type { AnalysisFields, MarketTab, MarketToken, SubmitResult } from "../../../shared/types";
import { integer } from "../../lib/format";
import { useApp } from "../../lib/store";
import { RiskBadge, SentimentBadge, TokenAvatar } from "../token/widgets";
import { Button } from "../ui/Button";
import { Badge, Banner, Card, MOD } from "../ui/primitives";

const TABS: MarketTab[] = ["all", "new", "live", "graduated"];

/** First market token that is not this one and not analyzed yet. */
export function useNextToken(mint: string) {
	const market = useApp((s) => s.market);
	const analyzed = useApp((s) => s.analyzed);
	const fetchMarket = useApp((s) => s.fetchMarket);
	const hasAny = TABS.some((t) => (market[t]?.tokens.length ?? 0) > 0);
	useEffect(() => {
		if (!hasAny) void fetchMarket("all");
	}, [hasAny, fetchMarket]);
	return useMemo(() => {
		for (const tab of TABS) {
			const t = market[tab]?.tokens.find((x) => x.mint !== mint && !analyzed[x.mint]);
			if (t) return t;
		}
		return null;
	}, [market, analyzed, mint]);
}

export function ResultPanel({
	symbol,
	result,
	fields,
	againLoading,
	next,
	marketLoading,
	onNext,
	onAgain,
	onBack,
}: {
	symbol: string;
	result: SubmitResult;
	fields: AnalysisFields;
	againLoading: boolean;
	next: MarketToken | null;
	marketLoading: boolean;
	onNext: (mint: string) => void;
	onAgain: () => void;
	onBack: () => void;
}) {
	const xp = result.xpEarned ?? 0;
	const validated = !!result.validated;
	const dev = result.deviationPct;

	const explanation =
		dev != null && Number.isFinite(dev)
			? `Your snapshot was ${dev.toFixed(1)}% off the live data; within ${VALIDATION_DEVIATION_PCT}% counts as validated.`
			: validated
				? `Your snapshot matched the live data within ${VALIDATION_DEVIATION_PCT}%.`
				: `The server could not confirm your snapshot within ${VALIDATION_DEVIATION_PCT}% of the live data.`;

	return (
		<div className="mx-auto w-full max-w-[640px] animate-pop-in px-6 py-10">
			<Card className="overflow-hidden">
				<div className="flex flex-col items-center px-6 pb-6 pt-8 text-center">
					<div className="mb-3 inline-flex items-center gap-1.5 text-sm text-fg-3">
						<CheckCircle2 className="size-4 text-pos" aria-hidden />
						Analysis of ${symbol} submitted
					</div>
					{xp > 0 ? (
						<div className="num text-hero font-semibold text-brand" aria-label={`${xp} XP earned`}>
							+{integer(xp)} XP
						</div>
					) : (
						<div className="text-hero font-semibold text-fg">Submitted</div>
					)}
					<div className="mt-4">
						{validated ? (
							<Badge tone="pos" icon={<CheckCircle2 className="size-3" aria-hidden />}>
								Validated
							</Badge>
						) : (
							<Badge tone="warn" icon={<AlertTriangle className="size-3" aria-hidden />}>
								Not validated
							</Badge>
						)}
					</div>
					<p className="mt-2 max-w-md text-base text-fg-2 [text-wrap:balance]">{explanation}</p>
					{result.xpTotal != null && (
						<p className="num mt-3 text-sm text-fg-3">
							Account total <span className="font-medium text-fg">{integer(result.xpTotal)} XP</span>
						</p>
					)}
				</div>

				{result.warning && (
					<div className="px-6 pb-5">
						<Banner tone="warn" title="Server note">
							{result.warning}
						</Banner>
					</div>
				)}

				<div className="flex flex-wrap items-center justify-center gap-2 border-t border-line bg-surface-1 px-6 py-3 text-sm text-fg-3">
					<span>You labelled it</span>
					<SentimentBadge value={fields.sentiment} score={fields.score} />
					<RiskBadge level={fields.riskLevel} />
					<span className="num">{fields.riskFactors.length} factors</span>
				</div>
			</Card>

			<div className="mt-5 flex flex-col gap-3">
				<Button
					variant="primary"
					size="lg"
					disabled={!next}
					loading={!next && marketLoading}
					onClick={() => next && onNext(next.mint)}
					iconRight={<ArrowRight className="size-4" />}
					kbd={`${MOD} ↵`}
					title={next ? `Open $${next.symbol}` : "No unanalyzed tokens in the market list. Browse tokens to pick one."}
					className="w-full"
				>
					{next ? (
						<span className="inline-flex min-w-0 items-center gap-2">
							Next token
							<TokenAvatar src={next.image_uri} symbol={next.symbol} mint={next.mint} size={18} />
							<span className="max-w-[160px] truncate font-medium opacity-80">${next.symbol}</span>
						</span>
					) : (
						"Next token"
					)}
				</Button>
				<div className="grid grid-cols-2 gap-3">
					<Button onClick={onAgain} loading={againLoading} icon={<RotateCcw className="size-4" />}>
						Analyze again
					</Button>
					<Button variant="ghost" onClick={onBack} icon={<ArrowLeft className="size-4" />}>
						Back to tokens
					</Button>
				</div>
				{!next && !marketLoading && (
					<p className="text-center text-sm text-fg-3">
						<Sparkles className="mr-1 inline size-3.5 align-[-2px] text-brand" aria-hidden />
						You analyzed every token in the current market list.
					</p>
				)}
			</div>
		</div>
	);
}
