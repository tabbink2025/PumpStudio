import { useState } from "react";
import { ArrowLeftRight, Globe, Radio, RefreshCw, Send } from "lucide-react";
import { SNAPSHOT_MAX_AGE_MS, VALIDATION_DEVIATION_PCT, buyRatio, top10Pct } from "../../../shared/analysis";
import type { DataPoint } from "../../../shared/types";
import { cn } from "../../lib/cn";
import { ago, compact, integer, pct, usdCompact, usdPrice } from "../../lib/format";
import { bun } from "../../lib/rpc";
import {
	Change,
	CopyMint,
	ExternalLinks,
	KV,
	Meter,
	PressureBar,
	TokenAvatar,
	TokenStatus,
	WatchButton,
	concentrationTone,
} from "../token/widgets";
import { Button, IconButton } from "../ui/Button";
import { Banner, Skeleton, Stat } from "../ui/primitives";

function Block({ title, children, className }: { title: string; children: React.ReactNode; className?: string }) {
	return (
		<section className={cn("border-t border-line px-4 py-4", className)}>
			<h3 className="mb-2.5 text-caption uppercase text-fg-3">{title}</h3>
			{children}
		</section>
	);
}

/** Left pane: what the analysis is about. */
export function TokenPanel({
	dp,
	snapshotAt,
	now,
	refreshing,
	onRefresh,
	onChangeToken,
}: {
	dp: DataPoint;
	snapshotAt: number;
	now: number;
	refreshing: boolean;
	onRefresh: () => void;
	onChangeToken: () => void;
}) {
	const [expanded, setExpanded] = useState(false);
	const stale = now - snapshotAt > SNAPSHOT_MAX_AGE_MS;
	const t10 = top10Pct(dp);
	const ratio = buyRatio(dp);
	const progress = dp.bondingComplete ? 100 : dp.bondingProgress;
	const socials = [
		dp.website ? { id: "web", label: "Website", url: dp.website, icon: <Globe className="size-3.5" aria-hidden /> } : null,
		dp.twitter ? { id: "x", label: "X", url: dp.twitter, icon: <XLogo /> } : null,
		dp.telegram ? { id: "tg", label: "Telegram", url: dp.telegram, icon: <Send className="size-3.5" aria-hidden /> } : null,
	].filter((s): s is NonNullable<typeof s> => !!s);
	const holdings: Array<[string, number | null | undefined, string]> = [
		["Creator holds", dp.creatorHolding, "Share of supply in the creator wallet"],
		["Snipers hold", dp.snipersHolding, "Share held by wallets that bought in the first blocks"],
		["Insiders hold", dp.insidersHolding, "Share held by wallets linked to the creator"],
		["Bundles hold", dp.bundleHolding, "Share bought in bundled transactions"],
		["Fresh wallets hold", dp.freshWalletsHolding, "Share held by wallets with no history"],
	];
	const shownHoldings = holdings.filter(([, v]) => typeof v === "number" && Number.isFinite(v));

	return (
		<div className="pb-6">
			{/* Header */}
			<div className="px-4 pb-4 pt-4">
				<div className="flex items-start gap-3">
					<TokenAvatar src={dp.imageUrl} symbol={dp.symbol} mint={dp.mint} size={40} progress={progress} />
					<div className="min-w-0 flex-1">
						<div className="flex items-center gap-1.5">
							<h2 className="truncate text-lg font-semibold text-fg" title={`$${dp.symbol}`}>
								${dp.symbol}
							</h2>
							<WatchButton mint={dp.mint} className="-my-1" />
						</div>
						<div className="truncate text-base text-fg-2" title={dp.name}>
							{dp.name}
						</div>
					</div>
				</div>
				<div className="mt-2 flex flex-wrap items-center gap-1.5">
					<CopyMint mint={dp.mint} className="-ml-1" />
					<TokenStatus live={dp.isLive} graduated={dp.bondingComplete} progress={dp.bondingProgress} />
				</div>
				<Button variant="ghost" size="sm" className="-ml-2.5 mt-2" icon={<ArrowLeftRight className="size-3.5" />} onClick={onChangeToken}>
					Change token
				</Button>
			</div>

			{/* Snapshot age */}
			<div className="flex items-center justify-between gap-2 border-t border-line px-4 py-2">
				<span className={cn("num text-sm", stale ? "text-warn" : "text-fg-3")} title={new Date(snapshotAt).toLocaleString()}>
					{refreshing ? "Refreshing snapshot…" : `Snapshot ${ago(snapshotAt, now)}`}
				</span>
				<IconButton label="Refresh snapshot" size="sm" onClick={onRefresh} disabled={refreshing}>
					<RefreshCw className={cn("size-3.5", refreshing && "animate-spin")} />
				</IconButton>
			</div>
			{stale && (
				<div className="px-4 pb-3">
					<Banner tone="warn" title="Snapshot is over 5 minutes old">
						<span className="text-sm">
							The server compares your snapshot with live data and only validates within {VALIDATION_DEVIATION_PCT}%. It is
							refreshed automatically when you submit.
						</span>
					</Banner>
				</div>
			)}

			{/* Key numbers */}
			<div className="grid grid-cols-2 gap-x-4 gap-y-3 border-t border-line px-4 py-4">
				<Stat label="Price" value={usdPrice(dp.priceUsd)} />
				<Stat label="24h change" value={<Change value={dp.priceChange24h} />} />
				<Stat label="Market cap" value={usdCompact(dp.marketCap)} />
				<Stat label="Liquidity" value={usdCompact(dp.liquidity)} />
				<Stat label="Volume 24h" value={usdCompact(dp.volume24h)} />
				<Stat label="Holders" value={compact(dp.holderCount)} title={integer(dp.holderCount)} />
				<Stat
					label="Top 10 hold"
					value={
						<span className="inline-flex items-center gap-1.5">
							{pct(t10)}
							{t10 != null && (
								<span className="text-sm font-normal">
									{t10 > 50 ? "high" : t10 > 30 ? "medium" : "low"}
								</span>
							)}
						</span>
					}
					tone={concentrationTone(t10) === "pos" ? undefined : concentrationTone(t10)}
					title="Share of supply held by the 10 largest wallets"
				/>
				<Stat label="Buy ratio" value={pct(ratio, 0)} title="Share of buys in 24h trades" />
			</div>

			<Block title="Bonding curve">
				<div className="mb-1.5 flex items-baseline justify-between text-sm">
					<span className="text-fg-2">{dp.bondingComplete ? "Graduated to a DEX" : "On the bonding curve"}</span>
					<span className="num text-fg">{pct(progress, 0)}</span>
				</div>
				<Meter
					value={progress ?? 0}
					tone={dp.bondingComplete ? "info" : (progress ?? 0) >= 70 ? "warn" : "brand"}
					label="Bonding curve progress"
				/>
			</Block>

			<Block title="Trades 24h">
				<PressureBar buys={dp.buys24h} sells={dp.sells24h} />
			</Block>

			<Block title="Price change">
				<div className="grid grid-cols-3 gap-2">
					{(
						[
							["1h", dp.priceChange1h],
							["6h", dp.priceChange6h],
							["24h", dp.priceChange24h],
						] as const
					).map(([label, v]) => (
						<div key={label} className="rounded-md border border-line bg-surface-2 px-2 py-1.5">
							<div className="text-caption uppercase text-fg-3">{label}</div>
							<Change value={v} className="text-base font-medium" />
						</div>
					))}
				</div>
			</Block>

			{shownHoldings.length > 0 && (
				<Block title="Holder breakdown">
					{shownHoldings.map(([label, v, hint]) => (
						<KV key={label} label={label} title={hint}>
							{pct(v)}
						</KV>
					))}
				</Block>
			)}

			<Block title="Socials">
				{socials.length === 0 ? (
					<p className="text-sm text-fg-3">No website, X or Telegram listed.</p>
				) : (
					<div className="flex flex-wrap gap-1.5">
						{socials.map((s) => (
							<button
								key={s.id}
								type="button"
								title={s.url}
								onClick={() => void bun.openExternal(s.url)}
								className="inline-flex h-7 items-center gap-1.5 rounded-md border border-line-strong bg-surface-3 px-2 text-sm text-fg-2 transition-colors hover:border-line-control hover:text-fg"
							>
								{s.icon}
								{s.label}
							</button>
						))}
					</div>
				)}
				{dp.isLive && (
					<p className="mt-2 inline-flex items-center gap-1.5 text-sm text-fg-2">
						<Radio className="size-3.5 text-neg" aria-hidden />
						Livestreaming
						{dp.viewerCount != null && <span className="num text-fg-3">· {integer(dp.viewerCount)} viewers</span>}
					</p>
				)}
			</Block>

			<Block title="Open in">
				<ExternalLinks mint={dp.mint} />
			</Block>

			{dp.description && (
				<Block title="Description">
					<p className="mb-1.5 text-sm text-fg-3">Written by the creator, not verified.</p>
					<p className={cn("selectable whitespace-pre-line break-words text-base text-fg-2", !expanded && "line-clamp-4")}>
						{dp.description}
					</p>
					{dp.description.length > 180 && (
						<button
							type="button"
							onClick={() => setExpanded((v) => !v)}
							className="mt-1 rounded text-sm font-medium text-info hover:underline"
							aria-expanded={expanded}
						>
							{expanded ? "Show less" : "Show more"}
						</button>
					)}
				</Block>
			)}
		</div>
	);
}

function XLogo() {
	return (
		<svg viewBox="0 0 24 24" className="size-3" fill="currentColor" aria-hidden>
			<path d="M18.244 2.25h3.308l-7.227 8.26 8.502 11.24H16.17l-5.214-6.817L4.99 21.75H1.68l7.73-8.835L1.254 2.25H8.08l4.713 6.231zm-1.161 17.52h1.833L7.084 4.126H5.117z" />
		</svg>
	);
}

/** Skeleton shaped like the token panel. */
export function TokenPanelSkeleton() {
	return (
		<div className="px-4 py-4" aria-busy aria-label="Loading token">
			<div className="flex items-center gap-3">
				<Skeleton className="size-10 rounded-full" />
				<div className="flex-1 space-y-2">
					<Skeleton className="h-4 w-24" />
					<Skeleton className="h-3 w-36" />
				</div>
			</div>
			<Skeleton className="mt-4 h-3 w-28" />
			<div className="mt-6 grid grid-cols-2 gap-x-4 gap-y-4">
				{Array.from({ length: 8 }, (_, i) => (
					<div key={i} className="space-y-1.5">
						<Skeleton className="h-2.5 w-14" />
						<Skeleton className="h-4 w-20" />
					</div>
				))}
			</div>
			<Skeleton className="mt-6 h-2 w-full" />
			<Skeleton className="mt-6 h-2 w-full" />
			<div className="mt-6 grid grid-cols-3 gap-2">
				<Skeleton className="h-10" />
				<Skeleton className="h-10" />
				<Skeleton className="h-10" />
			</div>
		</div>
	);
}
