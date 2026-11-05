/* ------------------------------------------------------------------ */
/*  Deterministic token analysis + validation, shared by Bun and view   */
/*                                                                      */
/*  The heuristics are ported from the official reference agent         */
/*  (pumpdotstudio/pump-quant, MIT) so that the pre-filled values match */
/*  what the Pump Studio fleet submits. Pure functions, no imports      */
/*  from electrobun, safe in both runtimes.                             */
/* ------------------------------------------------------------------ */

import type {
	AnalysisFields,
	AnalysisPayload,
	DataPoint,
	HolderConcentration,
	LiquidityDepth,
	Quant,
	RiskFactor,
	RiskLevel,
	Sentiment,
	Snapshot,
	TrendDirection,
	VolumeProfile,
} from "./types";

/* ---- Enums + metadata -------------------------------------------- */

export const SENTIMENTS: readonly Sentiment[] = ["bullish", "neutral", "bearish"];
export const RISK_LEVELS: readonly RiskLevel[] = ["low", "medium", "high", "critical"];
export const LIQUIDITY_DEPTHS: readonly LiquidityDepth[] = ["deep", "moderate", "shallow", "dry"];
export const HOLDER_CONCENTRATIONS: readonly HolderConcentration[] = [
	"distributed",
	"moderate",
	"concentrated",
	"whale_dominated",
];
export const TREND_DIRECTIONS: readonly TrendDirection[] = ["up", "sideways", "down", "reversal"];
export const VOLUME_PROFILES: readonly VolumeProfile[] = ["surging", "rising", "stable", "declining", "dead"];

export interface RiskFactorInfo {
	id: RiskFactor;
	label: string;
	kind: "negative" | "positive";
	hint: string;
}

export const RISK_FACTORS: readonly RiskFactorInfo[] = [
	{ id: "whale_dominance", label: "Whale dominance", kind: "negative", hint: "Top 10 wallets hold more than 80% of supply" },
	{ id: "high_concentration", label: "High concentration", kind: "negative", hint: "Top 10 wallets hold 50-80% of supply" },
	{ id: "single_holder_majority", label: "Single holder majority", kind: "negative", hint: "One wallet holds more than half the supply" },
	{ id: "creator_holds_majority", label: "Creator holds majority", kind: "negative", hint: "The creator wallet holds a large share" },
	{ id: "low_liquidity", label: "Low liquidity", kind: "negative", hint: "Under $50K liquidity: exits move the price a lot" },
	{ id: "no_liquidity_lock", label: "No liquidity lock", kind: "negative", hint: "Liquidity can be pulled" },
	{ id: "bonding_curve_risk", label: "Bonding curve risk", kind: "negative", hint: "Early on the bonding curve (under 30%)" },
	{ id: "rug_pattern", label: "Rug pattern", kind: "negative", hint: "Trading pattern resembles a rug pull" },
	{ id: "honeypot_risk", label: "Honeypot risk", kind: "negative", hint: "Selling may be blocked or taxed" },
	{ id: "wash_trading", label: "Wash trading", kind: "negative", hint: "Volume looks self-traded (insiders active)" },
	{ id: "fake_volume", label: "Fake volume", kind: "negative", hint: "Volume does not match holder activity" },
	{ id: "rapid_sell_off", label: "Rapid sell-off", kind: "negative", hint: "Sells outnumber buys more than 2 to 1" },
	{ id: "supply_manipulation", label: "Supply manipulation", kind: "negative", hint: "Supply changed or is controlled" },
	{ id: "dev_wallet_active", label: "Dev wallet active", kind: "negative", hint: "Developer or sniper wallets are trading" },
	{ id: "new_deployer", label: "New deployer", kind: "negative", hint: "Creator wallet has no history" },
	{ id: "copy_token", label: "Copy token", kind: "negative", hint: "Name or image copies another token" },
	{ id: "no_social_presence", label: "No socials", kind: "negative", hint: "No website, X or Telegram" },
	{ id: "no_website", label: "No website", kind: "negative", hint: "No website listed" },
	{ id: "declining_holders", label: "Declining holders", kind: "negative", hint: "Holders or price falling fast" },
	{ id: "dead_volume", label: "Dead volume", kind: "negative", hint: "Under $1K volume despite many holders" },
	{ id: "healthy_distribution", label: "Healthy distribution", kind: "positive", hint: "Top 10 hold 30% or less with 200+ holders" },
	{ id: "growing_holders", label: "Growing holders", kind: "positive", hint: "More than 500 holders" },
	{ id: "organic_volume", label: "Organic volume", kind: "positive", hint: "Strong volume with balanced buys" },
	{ id: "smart_money_inflow", label: "Smart money inflow", kind: "positive", hint: "Known profitable wallets are buying" },
	{ id: "strong_community", label: "Strong community", kind: "positive", hint: "Active community on X" },
	{ id: "verified_socials", label: "Verified socials", kind: "positive", hint: "X or Telegram linked" },
	{ id: "locked_liquidity", label: "Locked liquidity", kind: "positive", hint: "Liquidity is locked or burned" },
	{ id: "active_development", label: "Active development", kind: "positive", hint: "Project ships updates" },
];

export const RISK_FACTOR_IDS = RISK_FACTORS.map((f) => f.id) as readonly RiskFactor[];
const NEGATIVE = new Set(RISK_FACTORS.filter((f) => f.kind === "negative").map((f) => f.id));

export function riskFactorInfo(id: string): RiskFactorInfo | undefined {
	return RISK_FACTORS.find((f) => f.id === id);
}

export const SUMMARY_MIN = 50;
export const SUMMARY_MAX = 2000;
export const MAX_RISK_FACTORS = 8;
/** Snapshots older than this are refreshed before submitting (server checks deviation). */
export const SNAPSHOT_MAX_AGE_MS = 5 * 60_000;
/** Submissions are validated when the snapshot is within this % of live data. */
export const VALIDATION_DEVIATION_PCT = 15;

/* ---- Helpers ------------------------------------------------------ */

export function clamp(n: number, min: number, max: number): number {
	return Math.max(min, Math.min(max, n));
}

/** Finite number or fallback. */
export function num(n: unknown, fallback = 0): number {
	return typeof n === "number" && Number.isFinite(n) ? n : fallback;
}

/** Finite number or null (for display: "unknown" instead of 0). */
export function maybe(n: unknown): number | null {
	return typeof n === "number" && Number.isFinite(n) ? n : null;
}

export function top10Pct(dp: DataPoint): number | null {
	if (typeof dp.top10Holding === "number" && Number.isFinite(dp.top10Holding)) return dp.top10Holding;
	if (dp.topHolders && dp.topHolders.length > 0) {
		return dp.topHolders.slice(0, 10).reduce((sum, h) => sum + num(h.pct), 0);
	}
	return null;
}

/** Share of buys in 24h trades, 0-100, or null without trades. */
export function buyRatio(dp: DataPoint): number | null {
	const buys = num(dp.buys24h);
	const sells = num(dp.sells24h);
	if (buys + sells === 0) return null;
	return (buys / (buys + sells)) * 100;
}

export function buildSnapshot(dp: DataPoint, now = Date.now()): Snapshot {
	const t10 = top10Pct(dp) ?? 0;
	return {
		priceUsd: num(dp.priceUsd),
		marketCap: num(dp.marketCap),
		volume24h: num(dp.volume24h),
		liquidity: num(dp.liquidity),
		holderCount: num(dp.holderCount),
		top10HolderPct: Math.round(t10 * 10) / 10,
		buys24h: num(dp.buys24h),
		sells24h: num(dp.sells24h),
		bondingProgress: num(dp.bondingProgress),
		snapshotAt: dp.snapshotAt && now - dp.snapshotAt < SNAPSHOT_MAX_AGE_MS ? dp.snapshotAt : now,
	};
}

/* ---- Heuristics (pump-quant reference analyzer) ------------------- */

function computeBuyPressure(dp: DataPoint): number {
	const buys = num(dp.buys24h);
	const sells = num(dp.sells24h);
	if (buys + sells === 0) return 50;
	return clamp(Math.round((buys / (buys + sells + 1)) * 100), 0, 100);
}

function computeVolatilityScore(dp: DataPoint): number {
	const vol = num(dp.volume24h);
	const mcap = num(dp.marketCap, 1);
	if (mcap <= 0) return 50;
	const ratio = vol / mcap;
	if (ratio < 0.05) return clamp(Math.round(10 + ratio * 300), 10, 25);
	if (ratio < 0.2) return clamp(Math.round(25 + (ratio - 0.05) * 166), 25, 50);
	if (ratio < 0.5) return clamp(Math.round(50 + (ratio - 0.2) * 83), 50, 75);
	return clamp(Math.round(75 + (ratio - 0.5) * 40), 75, 95);
}

function computeLiquidityDepth(dp: DataPoint): LiquidityDepth {
	const liq = num(dp.liquidity);
	if (liq >= 1_000_000) return "deep";
	if (liq >= 250_000) return "moderate";
	if (liq >= 50_000) return "shallow";
	return "dry";
}

function computeHolderConcentration(t10: number): HolderConcentration {
	if (t10 > 80) return "whale_dominated";
	if (t10 > 50) return "concentrated";
	if (t10 > 20) return "moderate";
	return "distributed";
}

function computeTrendDirection(dp: DataPoint): TrendDirection {
	const has1h = typeof dp.priceChange1h === "number";
	const has24h = typeof dp.priceChange24h === "number";
	const ch1h = num(dp.priceChange1h);
	const ch24h = num(dp.priceChange24h);
	if (has1h || has24h) {
		if (ch1h > 5 && ch24h < -10) return "reversal";
		if (ch1h < -5 && ch24h > 10) return "reversal";
		const combined = ch1h * 0.6 + ch24h * 0.4;
		if (combined > 3) return "up";
		if (combined < -3) return "down";
		return "sideways";
	}
	const ratio = buyRatio(dp);
	if (ratio == null) return "sideways";
	if (ratio > 60) return "up";
	if (ratio < 40) return "down";
	return "sideways";
}

function computeVolumeProfile(dp: DataPoint): VolumeProfile {
	const vol = num(dp.volume24h);
	const mcap = num(dp.marketCap, 1);
	if (mcap <= 0) return "dead";
	const ratio = vol / mcap;
	if (ratio > 0.5) return "surging";
	if (ratio > 0.15) return "rising";
	if (ratio > 0.03) return "stable";
	if (ratio > 0.005) return "declining";
	return "dead";
}

function computeRiskFactors(dp: DataPoint, t10: number): RiskFactor[] {
	const f: RiskFactor[] = [];
	const buys = num(dp.buys24h);
	const sells = num(dp.sells24h);
	if (num(dp.liquidity) < 50_000) f.push("low_liquidity");
	if (t10 > 80) f.push("whale_dominance");
	if (t10 > 50 && t10 <= 80) f.push("high_concentration");
	if (dp.creatorHolding != null && dp.creatorHolding > 30) f.push("creator_holds_majority");
	if (dp.topHolders && dp.topHolders.length > 0 && num(dp.topHolders[0]!.pct) > 50) f.push("single_holder_majority");
	if (!dp.bondingComplete && num(dp.bondingProgress) < 30) f.push("bonding_curve_risk");
	if (!dp.website && !dp.twitter && !dp.telegram) f.push("no_social_presence");
	if (!dp.website) f.push("no_website");
	if (dp.snipersHolding != null && dp.snipersHolding > 20) f.push("dev_wallet_active");
	if (dp.insidersHolding != null && dp.insidersHolding > 15) f.push("wash_trading");
	if (sells > buys * 2 && sells > 20) f.push("rapid_sell_off");
	if (num(dp.volume24h) < 1000 && num(dp.holderCount) > 100) f.push("dead_volume");
	if (dp.priceChange24h != null && dp.priceChange24h < -20) f.push("declining_holders");
	if (t10 <= 30 && num(dp.holderCount) > 200) f.push("healthy_distribution");
	if (dp.twitter || dp.telegram) f.push("verified_socials");
	if (num(dp.volume24h) > 50_000 && buys > sells * 0.8) f.push("organic_volume");
	if (num(dp.holderCount) > 500) f.push("growing_holders");
	if (dp.twitter) f.push("strong_community");
	const deduped = [...new Set(f)];
	if (deduped.length === 0) deduped.push(num(dp.liquidity) >= 50_000 ? "organic_volume" : "low_liquidity");
	return deduped.slice(0, MAX_RISK_FACTORS);
}

export function riskLevelFor(factors: readonly RiskFactor[], buyPressure: number): RiskLevel {
	const negCount = factors.filter((x) => NEGATIVE.has(x)).length;
	const hasCritical = factors.some((x) => x === "rug_pattern" || x === "honeypot_risk" || x === "single_holder_majority");
	if (hasCritical || negCount >= 5) return "critical";
	if (negCount >= 3 || buyPressure < 25) return "high";
	if (negCount >= 1) return "medium";
	return "low";
}

function computeScore(q: Quant): number {
	let s = 50;
	s += (q.buyPressure - 50) * 0.5;
	if (q.trendDirection === "up") s += 10;
	else if (q.trendDirection === "down") s -= 10;
	else if (q.trendDirection === "reversal") s -= 5;
	if (q.volatilityScore > 70) s -= 5;
	if (q.riskLevel === "critical") s -= 20;
	else if (q.riskLevel === "high") s -= 10;
	else if (q.riskLevel === "low") s += 10;
	if (q.liquidityDepth === "deep") s += 5;
	else if (q.liquidityDepth === "dry") s -= 10;
	return clamp(Math.round(s), 0, 100);
}

export function sentimentFor(score: number, risk: RiskLevel): Sentiment {
	if (risk === "critical" || score < 35) return "bearish";
	if (score > 65 && risk !== "high") return "bullish";
	return "neutral";
}

export function compactNumber(n: number): string {
	const a = Math.abs(n);
	if (a >= 1_000_000_000) return `${(n / 1_000_000_000).toFixed(1)}B`;
	if (a >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
	if (a >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
	return n.toFixed(0);
}

function buildSummary(dp: DataPoint, sentiment: Sentiment, score: number, q: Quant, t10: number): string {
	const parts: string[] = [];
	if (dp.priceChange1h != null && Number.isFinite(dp.priceChange1h)) {
		parts.push(`Price ${dp.priceChange1h >= 0 ? "up" : "down"} ${Math.abs(dp.priceChange1h).toFixed(1)}% in the last hour`);
	} else if (dp.priceChange24h != null && Number.isFinite(dp.priceChange24h)) {
		parts.push(`Price ${dp.priceChange24h >= 0 ? "up" : "down"} ${Math.abs(dp.priceChange24h).toFixed(1)}% over 24h`);
	}
	const mcap = num(dp.marketCap);
	if (mcap > 0) parts.push(`mcap $${compactNumber(mcap)} with $${compactNumber(num(dp.volume24h))} 24h volume`);
	parts.push(`top 10 wallets hold ${t10.toFixed(1)}% (${q.holderConcentration.replace("_", " ")})`);
	parts.push(`liquidity ${q.liquidityDepth} at $${compactNumber(num(dp.liquidity))}`);
	parts.push(`${num(dp.buys24h)} buys vs ${num(dp.sells24h)} sells (${q.buyPressure}% buy pressure)`);
	const neg = q.riskFactors.filter((f) => NEGATIVE.has(f));
	if (neg.length > 0) parts.push(`risk factors: ${neg.map((f) => f.replace(/_/g, " ")).join(", ")}`);
	if (!dp.bondingComplete) parts.push(`bonding curve ${num(dp.bondingProgress).toFixed(0)}% complete`);
	return `${sentiment.toUpperCase()} (${score}/100). ${parts.join(". ")}.`;
}

/** Full deterministic analysis of a DataPoint. */
export function computeDefaults(dp: DataPoint): AnalysisFields {
	const t10 = top10Pct(dp) ?? 0;
	const buyPressure = computeBuyPressure(dp);
	const riskFactors = computeRiskFactors(dp, t10);
	const quant: Quant = {
		riskLevel: riskLevelFor(riskFactors, buyPressure),
		riskFactors,
		buyPressure,
		volatilityScore: computeVolatilityScore(dp),
		liquidityDepth: computeLiquidityDepth(dp),
		holderConcentration: computeHolderConcentration(t10),
		trendDirection: computeTrendDirection(dp),
		volumeProfile: computeVolumeProfile(dp),
	};
	const score = computeScore(quant);
	const sentiment = sentimentFor(score, quant.riskLevel);
	return { ...quant, sentiment, score, summary: buildSummary(dp, sentiment, score, quant, t10) };
}

/* ---- Validation --------------------------------------------------- */

function isOneOf<T extends string>(v: unknown, list: readonly T[]): v is T {
	return typeof v === "string" && (list as readonly string[]).includes(v);
}

/** Coerce arbitrary (e.g. AI-produced) values into valid fields, using fallback for anything invalid. */
export function sanitizeFields(raw: Partial<Record<keyof AnalysisFields, unknown>>, fallback: AnalysisFields): AnalysisFields {
	const n100 = (v: unknown, fb: number) =>
		typeof v === "number" && Number.isFinite(v) ? clamp(Math.round(v), 0, 100) : fb;
	const factors = Array.isArray(raw.riskFactors)
		? [...new Set(raw.riskFactors.filter((f): f is RiskFactor => isOneOf(f, RISK_FACTOR_IDS)))].slice(0, MAX_RISK_FACTORS)
		: [];
	const summary = typeof raw.summary === "string" ? raw.summary.trim().slice(0, SUMMARY_MAX) : "";
	return {
		sentiment: isOneOf(raw.sentiment, SENTIMENTS) ? raw.sentiment : fallback.sentiment,
		score: n100(raw.score, fallback.score),
		summary: summary.length >= SUMMARY_MIN ? summary : fallback.summary,
		riskLevel: isOneOf(raw.riskLevel, RISK_LEVELS) ? raw.riskLevel : fallback.riskLevel,
		riskFactors: factors.length > 0 ? factors : fallback.riskFactors,
		buyPressure: n100(raw.buyPressure, fallback.buyPressure),
		volatilityScore: n100(raw.volatilityScore, fallback.volatilityScore),
		liquidityDepth: isOneOf(raw.liquidityDepth, LIQUIDITY_DEPTHS) ? raw.liquidityDepth : fallback.liquidityDepth,
		holderConcentration: isOneOf(raw.holderConcentration, HOLDER_CONCENTRATIONS)
			? raw.holderConcentration
			: fallback.holderConcentration,
		trendDirection: isOneOf(raw.trendDirection, TREND_DIRECTIONS) ? raw.trendDirection : fallback.trendDirection,
		volumeProfile: isOneOf(raw.volumeProfile, VOLUME_PROFILES) ? raw.volumeProfile : fallback.volumeProfile,
	};
}

export type FieldErrors = Partial<Record<keyof AnalysisFields | "mint", string>>;

/** Client-side checks mirroring what the API accepts. Empty object = valid. */
export function validateFields(f: AnalysisFields): FieldErrors {
	const e: FieldErrors = {};
	const s = f.summary.trim();
	if (s.length < SUMMARY_MIN) e.summary = `Write at least ${SUMMARY_MIN} characters (${SUMMARY_MIN - s.length} to go)`;
	else if (s.length > SUMMARY_MAX) e.summary = `Keep it under ${SUMMARY_MAX} characters`;
	if (!Number.isInteger(f.score) || f.score < 0 || f.score > 100) e.score = "Score must be a whole number from 0 to 100";
	if (!Number.isInteger(f.buyPressure) || f.buyPressure < 0 || f.buyPressure > 100) e.buyPressure = "0 to 100";
	if (!Number.isInteger(f.volatilityScore) || f.volatilityScore < 0 || f.volatilityScore > 100) e.volatilityScore = "0 to 100";
	if (f.riskFactors.length === 0) e.riskFactors = "Pick at least one factor";
	else if (f.riskFactors.length > MAX_RISK_FACTORS) e.riskFactors = `Pick at most ${MAX_RISK_FACTORS}`;
	return e;
}

export function buildPayload(dp: DataPoint, f: AnalysisFields, now = Date.now()): AnalysisPayload {
	return {
		mint: dp.mint,
		sentiment: f.sentiment,
		score: clamp(Math.round(f.score), 0, 100),
		summary: f.summary.trim(),
		snapshot: buildSnapshot(dp, now),
		quant: {
			riskLevel: f.riskLevel,
			riskFactors: [...new Set(f.riskFactors)].slice(0, MAX_RISK_FACTORS),
			buyPressure: clamp(Math.round(f.buyPressure), 0, 100),
			volatilityScore: clamp(Math.round(f.volatilityScore), 0, 100),
			liquidityDepth: f.liquidityDepth,
			holderConcentration: f.holderConcentration,
			trendDirection: f.trendDirection,
			volumeProfile: f.volumeProfile,
		},
	};
}

/* ---- Mint parsing ------------------------------------------------- */

const BASE58 = /^[1-9A-HJ-NP-Za-km-z]{32,44}$/;

export function isMint(s: string): boolean {
	return BASE58.test(s.trim());
}

/**
 * Extract a mint from user input: a bare mint, or a pump.fun / pump.studio /
 * dexscreener / solscan / birdeye / gmgn URL that contains one.
 */
export function extractMint(input: string): string | null {
	const s = input.trim();
	if (isMint(s)) return s;
	const candidates = s.split(/[\s/?#=&]+/).filter(Boolean);
	for (const c of candidates.reverse()) if (isMint(c)) return c;
	return null;
}

export function shortMint(mint: string, head = 4, tail = 4): string {
	return mint.length <= head + tail + 1 ? mint : `${mint.slice(0, head)}…${mint.slice(-tail)}`;
}
