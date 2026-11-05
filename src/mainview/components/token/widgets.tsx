import { useState, type ReactNode } from "react";
import { ArrowDownRight, ArrowUpRight, Check, Copy, ExternalLink, GraduationCap, Minus, Radio, Star } from "lucide-react";
import { shortMint } from "../../../shared/analysis";
import type { RiskLevel, Sentiment } from "../../../shared/types";
import { cn } from "../../lib/cn";
import { pctSigned, titleCase } from "../../lib/format";
import { bun, errorMessage } from "../../lib/rpc";
import { toast, useApp } from "../../lib/store";
import { Badge, type Tone } from "../ui/primitives";

/* ---- Avatar ----------------------------------------------------------- */

function hue(s: string): number {
	let h = 0;
	for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) % 360;
	return h;
}

/** Token image with a generated fallback and an optional bonding-progress ring. */
export function TokenAvatar({
	src,
	symbol,
	mint,
	size = 32,
	progress,
	className,
}: {
	src?: string;
	symbol?: string;
	mint: string;
	size?: number;
	/** 0-100 bonding progress; draws a ring. 100 = graduated. */
	progress?: number | null;
	className?: string;
}) {
	const [failed, setFailed] = useState(false);
	const h = hue(mint);
	const letter = (symbol || "?").replace(/^\$/, "").slice(0, 1).toUpperCase();
	const ring = progress != null && Number.isFinite(progress);
	const inner = ring ? size - 6 : size;
	const r = (size - 2) / 2;
	const c = 2 * Math.PI * r;
	const pctVal = Math.max(0, Math.min(100, progress ?? 0));

	return (
		<span className={cn("relative inline-flex shrink-0 items-center justify-center", className)} style={{ width: size, height: size }}>
			{ring && (
				<svg className="absolute inset-0 -rotate-90" width={size} height={size} aria-hidden>
					<circle cx={size / 2} cy={size / 2} r={r} fill="none" strokeWidth={2} className="stroke-surface-4" />
					<circle
						cx={size / 2}
						cy={size / 2}
						r={r}
						fill="none"
						strokeWidth={2}
						strokeLinecap="round"
						strokeDasharray={c}
						strokeDashoffset={c * (1 - pctVal / 100)}
						className={pctVal >= 100 ? "stroke-info" : pctVal >= 70 ? "stroke-warn" : "stroke-brand"}
					/>
				</svg>
			)}
			{src && !failed ? (
				<img
					src={src}
					alt=""
					loading="lazy"
					referrerPolicy="no-referrer"
					onError={() => setFailed(true)}
					className="rounded-full bg-surface-3 object-cover"
					style={{ width: inner, height: inner }}
				/>
			) : (
				<span
					aria-hidden
					className="flex items-center justify-center rounded-full font-semibold text-white"
					style={{
						width: inner,
						height: inner,
						fontSize: Math.max(10, inner * 0.42),
						background: `linear-gradient(135deg, hsl(${h} 55% 42%), hsl(${(h + 40) % 360} 60% 30%))`,
					}}
				>
					{letter}
				</span>
			)}
		</span>
	);
}

/* ---- Copy mint -------------------------------------------------------- */

export function CopyMint({ mint, className, full }: { mint: string; className?: string; full?: boolean }) {
	const [copied, setCopied] = useState(false);
	async function copy(e: React.MouseEvent) {
		e.stopPropagation();
		try {
			await navigator.clipboard.writeText(mint);
		} catch {
			const ta = document.createElement("textarea");
			ta.value = mint;
			document.body.appendChild(ta);
			ta.select();
			document.execCommand("copy");
			ta.remove();
		}
		setCopied(true);
		setTimeout(() => setCopied(false), 1500);
		toast({ kind: "success", title: "Mint address copied", description: shortMint(mint, 6, 6), duration: 2500 });
	}
	return (
		<button
			type="button"
			onClick={copy}
			title={`Copy mint ${mint}`}
			aria-label="Copy mint address"
			className={cn(
				"group inline-flex min-w-0 items-center gap-1 rounded px-1 font-mono text-sm text-fg-3 transition-colors hover:bg-surface-3 hover:text-fg",
				className,
			)}
		>
			<span className="truncate">{full ? mint : shortMint(mint)}</span>
			{copied ? <Check className="size-3 shrink-0 text-pos" /> : <Copy className="size-3 shrink-0 opacity-60 group-hover:opacity-100" />}
		</button>
	);
}

/* ---- Watch star ------------------------------------------------------- */

export function WatchButton({ mint, className }: { mint: string; className?: string }) {
	const watching = useApp((s) => !!s.config?.prefs.watchlist.includes(mint));
	const toggle = useApp((s) => s.toggleWatch);
	return (
		<button
			type="button"
			aria-pressed={watching}
			aria-label={watching ? "Remove from watchlist" : "Add to watchlist"}
			title={watching ? "Remove from watchlist" : "Add to watchlist"}
			onClick={(e) => {
				e.stopPropagation();
				toggle(mint);
			}}
			className={cn(
				"inline-flex size-7 shrink-0 items-center justify-center rounded-md transition-colors hover:bg-surface-3",
				watching ? "text-warn" : "text-fg-3 hover:text-fg",
				className,
			)}
		>
			<Star className="size-4" fill={watching ? "currentColor" : "none"} />
		</button>
	);
}

/* ---- Values ----------------------------------------------------------- */

/** Signed % change with arrow glyph (never color alone). */
export function Change({ value, className, digits = 1 }: { value: number | null | undefined; className?: string; digits?: number }) {
	if (value == null || !Number.isFinite(value)) return <span className={cn("num text-fg-3", className)}>—</span>;
	const up = value > 0;
	const down = value < 0;
	const Icon = up ? ArrowUpRight : down ? ArrowDownRight : Minus;
	return (
		<span className={cn("num inline-flex items-center gap-0.5", up ? "text-pos" : down ? "text-neg" : "text-fg-2", className)}>
			<Icon className="size-3.5 shrink-0" aria-hidden />
			{pctSigned(value, digits)}
		</span>
	);
}

export const SENTIMENT_TONE: Record<Sentiment, Tone> = { bullish: "pos", neutral: "warn", bearish: "neg" };
export const RISK_TONE: Record<RiskLevel, Tone> = { low: "pos", medium: "warn", high: "orange", critical: "neg" };

export function SentimentBadge({ value, score }: { value?: string; score?: number }) {
	if (!value) return null;
	const tone = SENTIMENT_TONE[value as Sentiment] ?? "neutral";
	const Icon = value === "bullish" ? ArrowUpRight : value === "bearish" ? ArrowDownRight : Minus;
	return (
		<Badge tone={tone} icon={<Icon className="size-3" aria-hidden />}>
			{titleCase(value)}
			{score != null && <span className="num opacity-80">· {score}</span>}
		</Badge>
	);
}

export function RiskBadge({ level }: { level: RiskLevel }) {
	return (
		<Badge tone={RISK_TONE[level]} dot>
			{titleCase(level)} risk
		</Badge>
	);
}

/** Status pills for a market token. */
export function TokenStatus({ live, graduated, progress }: { live?: boolean; graduated?: boolean; progress?: number | null }) {
	return (
		<>
			{live && (
				<Badge tone="neg" icon={<Radio className="size-3" aria-hidden />} title="Livestreaming now">
					Live
				</Badge>
			)}
			{graduated ? (
				<Badge tone="info" icon={<GraduationCap className="size-3" aria-hidden />} title="Bonding curve complete, trading on a DEX">
					Graduated
				</Badge>
			) : progress != null && progress >= 70 ? (
				<Badge tone="warn" title="Bonding curve above 70%: close to graduation">
					Final stretch
				</Badge>
			) : null}
		</>
	);
}

/* ---- Bars ------------------------------------------------------------- */

export function Meter({
	value,
	tone = "brand",
	className,
	label,
}: {
	value: number;
	tone?: "brand" | "pos" | "neg" | "warn" | "info" | "orange";
	className?: string;
	label?: string;
}) {
	const color = { brand: "bg-brand", pos: "bg-pos", neg: "bg-neg", warn: "bg-warn", info: "bg-info", orange: "bg-orange" }[tone];
	const v = Math.max(0, Math.min(100, value));
	return (
		<div
			role="meter"
			aria-label={label}
			aria-valuenow={Math.round(v)}
			aria-valuemin={0}
			aria-valuemax={100}
			className={cn("h-1.5 overflow-hidden rounded-full bg-surface-4", className)}
		>
			<div className={cn("h-full rounded-full transition-[width] duration-slow", color)} style={{ width: `${v}%` }} />
		</div>
	);
}

/** Stacked buys vs sells bar. */
export function PressureBar({ buys, sells, className }: { buys: number | null | undefined; sells: number | null | undefined; className?: string }) {
	const b = buys ?? 0;
	const s = sells ?? 0;
	const total = b + s;
	const pctBuys = total > 0 ? (b / total) * 100 : 50;
	return (
		<div className={className}>
			<div className="flex h-1.5 overflow-hidden rounded-full bg-surface-4" title={`${b} buys / ${s} sells`}>
				<div className="h-full bg-pos" style={{ width: `${pctBuys}%` }} />
				<div className="h-full bg-neg" style={{ width: `${100 - pctBuys}%` }} />
			</div>
			<div className="num mt-1 flex justify-between text-sm">
				<span className="text-pos">{b.toLocaleString("en-US")} buys</span>
				<span className="text-neg">{s.toLocaleString("en-US")} sells</span>
			</div>
		</div>
	);
}

/** Tone for top-10 holder concentration (RugCheck-style tiers). */
export function concentrationTone(pctVal: number | null | undefined): "pos" | "warn" | "neg" | undefined {
	if (pctVal == null) return undefined;
	if (pctVal > 50) return "neg";
	if (pctVal > 30) return "warn";
	return "pos";
}

/* ---- External links --------------------------------------------------- */

export const EXTERNAL_LINKS: Array<{ id: string; label: string; url: (mint: string) => string }> = [
	{ id: "studio", label: "pump.studio", url: (m) => `https://pump.studio/datapoint?mint=${m}` },
	{ id: "pumpfun", label: "pump.fun", url: (m) => `https://pump.fun/coin/${m}` },
	{ id: "dexscreener", label: "DEX Screener", url: (m) => `https://dexscreener.com/solana/${m}` },
	{ id: "birdeye", label: "Birdeye", url: (m) => `https://birdeye.so/token/${m}?chain=solana` },
	{ id: "gmgn", label: "GMGN", url: (m) => `https://gmgn.ai/sol/token/${m}` },
	{ id: "solscan", label: "Solscan", url: (m) => `https://solscan.io/token/${m}` },
];

export function ExternalLinks({ mint, className }: { mint: string; className?: string }) {
	return (
		<div className={cn("flex flex-wrap gap-1.5", className)}>
			{EXTERNAL_LINKS.map((l) => (
				<button
					key={l.id}
					type="button"
					onClick={() =>
						bun.openExternal(l.url(mint)).catch((err) =>
							toast({ kind: "error", title: `Could not open ${l.label}`, description: errorMessage(err) }),
						)
					}
					className="inline-flex h-7 items-center gap-1 rounded-md border border-line-strong bg-surface-3 px-2 text-sm text-fg-2 transition-colors hover:border-line-control hover:text-fg"
				>
					{l.label}
					<ExternalLink className="size-3 opacity-70" aria-hidden />
				</button>
			))}
		</div>
	);
}

/** Label/value row for dense key-value lists. */
export function KV({ label, children, title }: { label: ReactNode; children: ReactNode; title?: string }) {
	return (
		<div className="flex items-center justify-between gap-3 py-1" title={title}>
			<span className="text-sm text-fg-3">{label}</span>
			<span className="num truncate text-base text-fg">{children}</span>
		</div>
	);
}
