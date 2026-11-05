import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { AtSign, CheckCircle2, ExternalLink, Globe, GraduationCap, Send, X, Zap } from "lucide-react";
import { buyRatio, computeDefaults, RISK_FACTORS, top10Pct } from "../../../shared/analysis";
import type { DataPoint, MarketToken } from "../../../shared/types";
import { cn } from "../../lib/cn";
import { absoluteTime, ago, integer, pct, usdCompact, usdPrice } from "../../lib/format";
import { bun, errorMessage } from "../../lib/rpc";
import { toast, useApp } from "../../lib/store";
import {
	Change,
	concentrationTone,
	CopyMint,
	ExternalLinks,
	Meter,
	PressureBar,
	RiskBadge,
	SentimentBadge,
	TokenAvatar,
	TokenStatus,
	WatchButton,
} from "../token/widgets";
import { Button, IconButton } from "../ui/Button";
import { Badge, ErrorState, SectionTitle, Skeleton, Stat } from "../ui/primitives";
import { socialUrl } from "./model";

/* ---- DataPoint fetching: 30s cache per mint, race safe ------------------ */

const DP_TTL_MS = 30_000;
const dpCache = new Map<string, { dp: DataPoint; at: number }>();

interface DpState {
	mint: string;
	dp?: DataPoint;
	error?: string;
	loading: boolean;
}

/** Open a link in the system browser; failures become a toast, never a silent rejection. */
function openLink(open: () => Promise<unknown>) {
	open().catch((err) => toast({ kind: "error", title: "Could not open the link", description: errorMessage(err) }));
}

function useDataPoint(mint: string, reloadSignal: number) {
	const [state, setState] = useState<DpState>(() => {
		const hit = dpCache.get(mint);
		return { mint, dp: hit?.dp, loading: !hit };
	});
	const reqId = useRef(0);
	const rememberTokens = useApp((s) => s.rememberTokens);

	const load = useCallback(
		(force = false) => {
			const id = ++reqId.current;
			const hit = dpCache.get(mint);
			if (hit && !force && Date.now() - hit.at < DP_TTL_MS) {
				setState({ mint, dp: hit.dp, loading: false });
				return;
			}
			// Keep showing a cached snapshot for the same mint while refreshing.
			setState({ mint, dp: hit?.dp, loading: true });
			bun.getDataPoint(mint).then(
				(dp) => {
					dpCache.set(mint, { dp, at: Date.now() });
					rememberTokens([{ mint, symbol: dp.symbol, name: dp.name, image: dp.imageUrl }]);
					if (id === reqId.current) setState({ mint, dp, loading: false });
				},
				(err) => {
					if (id === reqId.current) setState({ mint, dp: hit?.dp, loading: false, error: errorMessage(err) });
				},
			);
		},
		[mint, rememberTokens],
	);

	useEffect(() => {
		load();
	}, [load]);

	// Header refresh (button, Ctrl+R): fetch a fresh snapshot for the open token.
	const lastSignal = useRef(reloadSignal);
	useEffect(() => {
		if (reloadSignal === lastSignal.current) return;
		lastSignal.current = reloadSignal;
		load(true);
	}, [reloadSignal, load]);

	// Never show the previous token's data for a moment after switching.
	const current = state.mint === mint ? state : { mint, loading: true };
	return { ...current, reload: () => load(true) };
}

/* ---- Small layout pieces ------------------------------------------------ */

function Section({ title, action, children, className }: { title: string; action?: ReactNode; children: ReactNode; className?: string }) {
	return (
		<section className={cn("border-t border-line px-4 py-4", className)}>
			<SectionTitle action={action} className="mb-2.5">
				{title}
			</SectionTitle>
			{children}
		</section>
	);
}

const CONCENTRATION_LABEL = { pos: "Spread out", warn: "Moderate", neg: "Concentrated" } as const;

function StatCell({ children }: { children: ReactNode }) {
	return <div className="min-w-0 bg-surface-1 px-3 py-2.5">{children}</div>;
}

function SocialButton({ icon, label, url }: { icon: ReactNode; label: string; url: string }) {
	return (
		<button
			type="button"
			onClick={() => openLink(() => bun.openExternal(url))}
			title={url}
			className="inline-flex h-7 min-w-0 items-center gap-1.5 rounded-md border border-line-strong bg-surface-3 px-2 text-sm text-fg-2 transition-colors hover:border-line-control hover:text-fg"
		>
			<span className="shrink-0 text-fg-3" aria-hidden>
				{icon}
			</span>
			{label}
		</button>
	);
}

/* ---- Inspector ---------------------------------------------------------- */

export function TokenInspector({
	mint,
	token,
	reloadSignal = 0,
	onClose,
}: {
	mint: string;
	token?: MarketToken;
	/** Changes when the screen refreshes; the inspector then refetches its snapshot. */
	reloadSignal?: number;
	onClose: () => void;
}) {
	const { dp, error, loading, reload } = useDataPoint(mint, reloadSignal);
	const meta = useApp((s) => s.tokenMeta[mint]);
	const mine = useApp((s) => s.analyzed[mint]);
	const apiKeySet = useApp((s) => !!s.config?.apiKeySet);
	const openAnalyze = useApp((s) => s.openAnalyze);

	const symbol = dp?.symbol || token?.symbol || meta?.symbol || "";
	const name = dp?.name || token?.name || meta?.name || "";
	const image = dp?.imageUrl || token?.image_uri || meta?.image;
	const graduated = dp ? dp.bondingComplete : token?.complete === true;
	const live = dp?.isLive ?? token?.is_currently_live;
	const progress = graduated ? 100 : (dp?.bondingProgress ?? null);
	const label = symbol ? `$${symbol}` : "Unknown token";

	const quick = useMemo(() => (dp ? computeDefaults(dp) : null), [dp]);
	const negFactors = useMemo(
		() => (quick ? RISK_FACTORS.filter((f) => f.kind === "negative" && quick.riskFactors.includes(f.id)) : []),
		[quick],
	);
	const posFactors = useMemo(
		() => (quick ? RISK_FACTORS.filter((f) => f.kind === "positive" && quick.riskFactors.includes(f.id)) : []),
		[quick],
	);

	const [moreDesc, setMoreDesc] = useState(false);
	useEffect(() => setMoreDesc(false), [mint]);

	const description = (dp?.description ?? token?.description ?? "").trim();
	const socials = dp
		? [
				{ id: "website", icon: <Globe className="size-3.5" />, label: "Website", url: socialUrl("website", dp.website) },
				{ id: "twitter", icon: <AtSign className="size-3.5" />, label: "X", url: socialUrl("twitter", dp.twitter) },
				{ id: "telegram", icon: <Send className="size-3.5" />, label: "Telegram", url: socialUrl("telegram", dp.telegram) },
			].filter((s): s is { id: string; icon: JSX.Element; label: string; url: string } => !!s.url)
		: [];

	const t10 = dp ? top10Pct(dp) : null;
	const t10Tone = concentrationTone(t10);
	const ratio = dp ? buyRatio(dp) : null;
	const ratioTone = ratio == null ? undefined : ratio >= 55 ? "pos" : ratio <= 45 ? "neg" : undefined;

	return (
		<aside
			aria-label={`Details for ${label}`}
			className="flex w-[340px] shrink-0 animate-slide-in flex-col border-l border-line bg-surface-1 xl:w-[380px]"
		>
			{/* Header */}
			<div className="shrink-0 border-b border-line px-4 pb-3 pt-4">
				<div className="flex items-start gap-3">
					<TokenAvatar src={image} symbol={symbol} mint={mint} size={40} progress={progress} />
					<div className="min-w-0 flex-1">
						<div className="flex items-baseline gap-2">
							<h2 className="truncate text-md font-semibold text-fg" title={label}>
								{label}
							</h2>
						</div>
						<div className="truncate text-sm text-fg-3" title={name || undefined}>
							{name || "Name unknown"}
						</div>
					</div>
					<div className="-mr-1.5 -mt-1 flex shrink-0 items-center">
						<WatchButton mint={mint} />
						<IconButton size="sm" label="Close details (Esc)" onClick={onClose}>
							<X className="size-4" aria-hidden />
						</IconButton>
					</div>
				</div>
				<div className="mt-2.5 flex flex-wrap items-center gap-1.5">
					<CopyMint mint={mint} className="-ml-1" />
					<TokenStatus live={live} graduated={graduated} progress={progress} />
				</div>
			</div>

			{/* Body */}
			<div className="min-h-0 flex-1 overflow-y-auto" aria-busy={loading || undefined}>
				{!dp && error ? (
					<div>
						<ErrorState title="Could not load token data" message={error} onRetry={reload} />
					</div>
				) : !dp ? (
					<InspectorSkeleton />
				) : (
					<>
						{/* Price */}
						<div className="flex items-end justify-between gap-3 px-4 py-4">
							<div className="min-w-0">
								<div className="text-caption uppercase text-fg-3">Price</div>
								<div className="num mt-0.5 truncate text-kpi font-semibold text-fg">{usdPrice(dp.priceUsd)}</div>
							</div>
							<div className="shrink-0 pb-1 text-right">
								<Change value={dp.priceChange24h} className="text-md font-medium" />
								<div className="text-sm text-fg-3">24h</div>
							</div>
						</div>

						{error && (
							<div className="-mt-1 px-4 pb-3 text-sm text-warn" role="status">
								Could not refresh: {error}.{" "}
								<button type="button" onClick={reload} className="rounded underline underline-offset-2 hover:text-fg">
									Try again
								</button>
							</div>
						)}

						{/* Metrics */}
						<div className="px-4 pb-4">
							<div className="grid grid-cols-2 gap-px overflow-hidden rounded-lg border border-line bg-line">
								<StatCell>
									<Stat label="Market cap" value={usdCompact(dp.marketCap)} />
								</StatCell>
								<StatCell>
									<Stat label="Liquidity" value={usdCompact(dp.liquidity)} />
								</StatCell>
								<StatCell>
									<Stat label="Volume 24h" value={usdCompact(dp.volume24h)} />
								</StatCell>
								<StatCell>
									<Stat label="Holders" value={integer(dp.holderCount)} />
								</StatCell>
								<StatCell>
									<Stat
										label="Top 10 holders"
										value={pct(t10, 1)}
										tone={t10Tone}
										sub={t10Tone ? CONCENTRATION_LABEL[t10Tone] : apiKeySet ? "Not reported" : "Needs an API key"}
										title="Share of supply held by the 10 largest wallets"
									/>
								</StatCell>
								<StatCell>
									<Stat
										label="Buy pressure"
										value={pct(ratio, 0)}
										tone={ratioTone}
										sub={ratio == null ? "No trades in 24h" : ratio >= 55 ? "Buyers lead" : ratio <= 45 ? "Sellers lead" : "Balanced"}
										title="Share of buys in the last 24h of trades"
									/>
								</StatCell>
							</div>
						</div>

						<Section
							title="Bonding curve"
							action={
								graduated ? (
									<span className="inline-flex items-center gap-1 text-sm font-medium text-info">
										<GraduationCap className="size-3.5" aria-hidden />
										Graduated
									</span>
								) : (
									<span className="num text-sm font-medium text-fg">{pct(dp.bondingProgress, 0)}</span>
								)
							}
						>
							<Meter
								value={graduated ? 100 : (dp.bondingProgress ?? 0)}
								tone={graduated ? "info" : (dp.bondingProgress ?? 0) >= 70 ? "warn" : "brand"}
								label="Bonding curve progress"
							/>
							<p className="mt-1.5 text-sm text-fg-3">
								{graduated ? "Curve complete, trading on a DEX." : "Graduates to a DEX at 100%."}
							</p>
						</Section>

						<Section title="Trades 24h">
							<PressureBar buys={dp.buys24h} sells={dp.sells24h} />
						</Section>

						<Section title="Price change">
							<div className="grid grid-cols-3 gap-2">
								{(
									[
										["1h", dp.priceChange1h],
										["6h", dp.priceChange6h],
										["24h", dp.priceChange24h],
									] as const
								).map(([k, v]) => (
									<div key={k} className="rounded-md border border-line bg-surface-2 px-2.5 py-2">
										<div className="text-sm text-fg-3">{k}</div>
										<Change value={v} className="text-base font-medium" />
									</div>
								))}
							</div>
						</Section>

						{quick && (
							<Section title="Quick read" action={<span className="text-sm text-fg-3">Rule-based, before your own analysis</span>}>
								<div className="flex flex-wrap items-center gap-1.5">
									<RiskBadge level={quick.riskLevel} />
									<SentimentBadge value={quick.sentiment} score={quick.score} />
								</div>
								{negFactors.length > 0 ? (
									<div className="mt-2.5 flex flex-wrap gap-1">
										{negFactors.map((f) => (
											<Badge key={f.id} tone="neutral" title={f.hint} icon={<span className="size-1.5 rounded-full bg-neg" aria-hidden />}>
												{f.label}
											</Badge>
										))}
									</div>
								) : (
									<p className="mt-2 flex items-center gap-1.5 text-sm text-fg-2">
										<CheckCircle2 className="size-3.5 text-pos" aria-hidden />
										No red flags found in the snapshot
									</p>
								)}
								{posFactors.length > 0 && (
									<div className="mt-1.5 flex flex-wrap gap-1">
										{posFactors.map((f) => (
											<Badge key={f.id} tone="neutral" title={f.hint} icon={<span className="size-1.5 rounded-full bg-pos" aria-hidden />}>
												{f.label}
											</Badge>
										))}
									</div>
								)}
							</Section>
						)}

						{mine && (
							<Section title="Your last call" action={<span className="num text-sm text-fg-3" title={absoluteTime(mine.at)}>{ago(mine.at)}</span>}>
								<div className="flex flex-wrap items-center gap-1.5">
									<SentimentBadge value={mine.sentiment} score={mine.score} />
									{mine.validated != null && (
										<Badge tone={mine.validated ? "pos" : "warn"}>{mine.validated ? "Validated" : "Not validated"}</Badge>
									)}
									{mine.xp != null && mine.xp > 0 && <Badge tone="brand">+{integer(mine.xp)} XP</Badge>}
									{mine.count > 1 && <span className="num text-sm text-fg-3">{mine.count} analyses</span>}
								</div>
							</Section>
						)}

						{socials.length > 0 && (
							<Section title="Socials">
								<div className="flex flex-wrap gap-1.5">
									{socials.map((s) => (
										<SocialButton key={s.id} icon={s.icon} label={s.label} url={s.url} />
									))}
								</div>
							</Section>
						)}

						{description && (
							<Section title="About" action={<span className="text-sm text-fg-3">Written by the creator</span>}>
								<p className={cn("selectable whitespace-pre-line break-words text-base text-fg-2", !moreDesc && "line-clamp-4")}>
									{description}
								</p>
								{description.length > 180 && (
									<button
										type="button"
										onClick={() => setMoreDesc((v) => !v)}
										className="mt-1 rounded text-sm text-fg-3 underline underline-offset-2 hover:text-fg"
									>
										{moreDesc ? "Show less" : "Show more"}
									</button>
								)}
							</Section>
						)}

						<Section title="Open in">
							<ExternalLinks mint={mint} />
						</Section>

						{dp.snapshotAt && (
							<p className="num px-4 pb-4 text-sm text-fg-3" title={absoluteTime(dp.snapshotAt)}>
								Snapshot taken {ago(dp.snapshotAt)}
							</p>
						)}
					</>
				)}
			</div>

			{/* Footer */}
			<div className="flex shrink-0 items-center gap-2 border-t border-line bg-surface-1 px-4 py-3">
				<Button variant="primary" kbd="A" icon={<Zap className="size-4" aria-hidden />} className="flex-1" onClick={() => openAnalyze(mint)}>
					Analyze
				</Button>
				<Button
					variant="secondary"
					iconRight={<ExternalLink className="size-3.5 opacity-70" aria-hidden />}
					onClick={() => openLink(() => bun.openWebApp(`/datapoint?mint=${mint}`))}
				>
					Open on pump.studio
				</Button>
			</div>
		</aside>
	);
}

function InspectorSkeleton() {
	return (
		<div aria-hidden>
			<div className="flex items-end justify-between px-4 py-4">
				<div>
					<Skeleton className="h-3 w-10" />
					<Skeleton className="mt-2 h-7 w-32" />
				</div>
				<Skeleton className="mb-1 h-4 w-14" />
			</div>
			<div className="px-4 pb-4">
				<div className="grid grid-cols-2 gap-px overflow-hidden rounded-lg border border-line bg-line">
					{Array.from({ length: 6 }, (_, i) => (
						<div key={i} className="bg-surface-1 px-3 py-2.5">
							<Skeleton className="h-3 w-16" />
							<Skeleton className="mt-2 h-5 w-20" />
						</div>
					))}
				</div>
			</div>
			{[0, 1, 2].map((i) => (
				<div key={i} className="border-t border-line px-4 py-4">
					<Skeleton className="h-3 w-24" />
					<Skeleton className="mt-3 h-1.5 w-full rounded-full" />
					<Skeleton className="mt-2 h-3 w-40" />
				</div>
			))}
		</div>
	);
}
