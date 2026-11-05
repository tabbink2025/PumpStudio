/* ------------------------------------------------------------------ */
/*  Shared types between the Bun main process and the webview           */
/*                                                                      */
/*  Shapes follow the official Pump Studio clients                      */
/*  (github.com/pumpdotstudio/pump-quant and /intern). The API wraps    */
/*  every response in { ok, data, error } and most DataPoint numbers    */
/*  can be null, so every consumer must treat them as optional.         */
/* ------------------------------------------------------------------ */

export type MarketTab = "all" | "live" | "new" | "graduated";

export interface MarketToken {
	mint: string;
	name: string;
	symbol: string;
	description?: string;
	image_uri?: string;
	usd_market_cap?: number;
	market_cap?: number;
	created_timestamp?: number;
	is_currently_live?: boolean;
	complete?: boolean;
}

export interface TopHolder {
	address: string;
	amount: number;
	pct: number;
}

export interface RecentTrade {
	tx: string;
	type: "buy" | "sell";
	sol: number;
	wallet: string;
	timestamp: number;
}

/** Token snapshot from GET /api/v1/datapoint (subset of the 71 fields). */
export interface DataPoint {
	mint: string;
	name: string;
	symbol: string;
	description?: string;
	imageUrl?: string;
	creator?: string;
	priceUsd: number | null;
	priceSol?: number | null;
	solPriceUsd?: number | null;
	priceChange1h?: number | null;
	priceChange6h?: number | null;
	priceChange24h: number | null;
	marketCap: number | null;
	fdv?: number | null;
	totalSupply?: number | null;
	liquidity: number | null;
	volume1m?: number | null;
	volume1h?: number | null;
	volume6h?: number | null;
	volume24h: number | null;
	buys24h: number | null;
	sells24h: number | null;
	tradeCount?: number | null;
	holderCount: number | null;
	tradeRate1m?: number | null;
	buySellImbalance1m?: number | null;
	bondingComplete: boolean;
	bondingProgress: number | null;
	isLive?: boolean;
	streamSource?: "pumpfun" | "studio" | "none";
	viewerCount?: number | null;
	website?: string | null;
	twitter?: string | null;
	telegram?: string | null;
	dexPaid?: boolean;
	dexBoosted?: boolean;
	primaryDex?: string | null;
	/* keyed tier (only with an API key) */
	topHolders?: TopHolder[];
	top10Holding?: number | null;
	snipersHolding?: number | null;
	insidersHolding?: number | null;
	bundleHolding?: number | null;
	freshWalletsHolding?: number | null;
	creatorHolding?: number | null;
	recentTrades?: RecentTrade[];
	snapshotAt?: number;
	ttl?: number;
	version?: string;
	[key: string]: unknown;
}

/* ---- Analysis enums --------------------------------------------- */

export type Sentiment = "bullish" | "bearish" | "neutral";
export type RiskLevel = "critical" | "high" | "medium" | "low";
export type LiquidityDepth = "deep" | "moderate" | "shallow" | "dry";
export type HolderConcentration = "distributed" | "moderate" | "concentrated" | "whale_dominated";
export type TrendDirection = "up" | "down" | "sideways" | "reversal";
export type VolumeProfile = "surging" | "rising" | "stable" | "declining" | "dead";

export type RiskFactor =
	| "whale_dominance" | "creator_holds_majority" | "low_liquidity" | "no_liquidity_lock"
	| "high_concentration" | "rug_pattern" | "honeypot_risk" | "wash_trading"
	| "bonding_curve_risk" | "rapid_sell_off" | "no_social_presence" | "fake_volume"
	| "supply_manipulation" | "dev_wallet_active" | "copy_token" | "no_website"
	| "new_deployer" | "single_holder_majority" | "declining_holders" | "dead_volume"
	| "healthy_distribution" | "strong_community" | "organic_volume" | "locked_liquidity"
	| "verified_socials" | "active_development" | "growing_holders" | "smart_money_inflow";

export interface Snapshot {
	priceUsd: number;
	marketCap: number;
	volume24h: number;
	liquidity: number;
	holderCount: number;
	top10HolderPct: number;
	buys24h: number;
	sells24h: number;
	bondingProgress: number;
	snapshotAt: number;
}

export interface Quant {
	riskLevel: RiskLevel;
	riskFactors: RiskFactor[];
	buyPressure: number;
	volatilityScore: number;
	liquidityDepth: LiquidityDepth;
	holderConcentration: HolderConcentration;
	trendDirection: TrendDirection;
	volumeProfile: VolumeProfile;
}

/** The labels a user (or AI) puts on a token. */
export interface AnalysisFields extends Quant {
	sentiment: Sentiment;
	score: number;
	summary: string;
}

export interface AnalysisPayload {
	mint: string;
	sentiment: Sentiment;
	score: number;
	summary: string;
	snapshot: Snapshot;
	quant: Quant;
}

export interface SubmitResult {
	ok: boolean;
	error?: string;
	xpEarned?: number;
	xpTotal?: number;
	analysisId?: string;
	validated?: boolean;
	deviationPct?: number;
	warning?: string;
}

export interface LeaderboardEntry {
	mint: string;
	name?: string;
	symbol?: string;
	image?: string;
	count: number;
	lastSentiment?: string;
	lastScore?: number;
}

export interface AnalysisStats {
	totalAnalyses: number;
	uniqueTokens: number;
	goal: number;
	progress: number;
	leaderboard: LeaderboardEntry[];
}

/* ---- AI orchestration -------------------------------------------- */

export type Orchestrator = "manual" | "claude" | "openai";

export interface OrchestratorResult extends AnalysisFields {
	/** Which engine actually produced the values. */
	source: Orchestrator | "heuristic";
	/** Model id when an AI produced the values. */
	model?: string;
	/** Set when the configured AI failed and heuristics were used instead. */
	fallbackReason?: string;
	durationMs: number;
}

/* ---- Configuration ------------------------------------------------ */

export type ThemePref = "system" | "dark" | "light";
export type Density = "compact" | "comfortable";

export interface UiPrefs {
	theme: ThemePref;
	density: Density;
	sidebarCollapsed: boolean;
	watchlist: string[];
	recentMints: string[];
}

/** Full config as stored on disk by the Bun process. Never sent to the view. */
export interface AgentConfig {
	apiKey: string;
	orchestrator: Orchestrator;
	claudeApiKey: string;
	openaiApiKey: string;
	claudeModel: string;
	openaiModel: string;
	analyzeInterval: number;
	skipRecentlyAnalyzed: boolean;
	prefs: UiPrefs;
}

/** What the view sees: secrets replaced by a "set" flag and a masked hint. */
export interface PublicConfig {
	apiKeySet: boolean;
	apiKeyHint: string;
	orchestrator: Orchestrator;
	claudeKeySet: boolean;
	claudeKeyHint: string;
	openaiKeySet: boolean;
	openaiKeyHint: string;
	claudeModel: string;
	openaiModel: string;
	analyzeInterval: number;
	skipRecentlyAnalyzed: boolean;
	prefs: UiPrefs;
	configPath: string;
}

/**
 * A config update from the view. Secret fields: a string sets the key,
 * null clears it, undefined leaves it unchanged.
 */
export interface ConfigPatch {
	apiKey?: string | null;
	claudeApiKey?: string | null;
	openaiApiKey?: string | null;
	orchestrator?: Orchestrator;
	claudeModel?: string;
	openaiModel?: string;
	analyzeInterval?: number;
	skipRecentlyAnalyzed?: boolean;
	prefs?: Partial<UiPrefs>;
}

export type KeyCheck =
	| { status: "valid"; agentName?: string }
	| { status: "invalid"; message: string }
	| { status: "unreachable"; message: string }
	| { status: "missing" };

export interface RegisteredKey {
	key: string;
	type?: string;
	rateLimit?: number;
}

/* ---- Auto-training ----------------------------------------------- */

export type TrainingStep = "queued" | "fetching" | "analyzing" | "submitting" | "done" | "skipped" | "error";

export interface TrainingItem {
	mint: string;
	symbol?: string;
	name?: string;
	image?: string;
	step: TrainingStep;
	sentiment?: Sentiment;
	score?: number;
	xpEarned?: number;
	validated?: boolean;
	deviationPct?: number;
	source?: OrchestratorResult["source"];
	error?: string;
	/** Pipeline stage that failed, when step is "error". */
	failedStep?: "fetching" | "analyzing" | "submitting";
	updatedAt: number;
}

export interface TrainingState {
	status: "idle" | "running" | "stopping";
	runId: number;
	startedAt?: number;
	finishedAt?: number;
	total: number;
	completed: number;
	failed: number;
	skipped: number;
	xpEarned: number;
	validatedCount: number;
	intervalSec: number;
	orchestrator: Orchestrator;
	items: TrainingItem[];
	/** Epoch ms when the loop will process the next token (during the wait). */
	nextAt?: number;
	lastError?: string;
}

export interface TrainingTarget {
	mint: string;
	symbol?: string;
	name?: string;
	image?: string;
}

/* ---- Session XP --------------------------------------------------- */

export interface XpState {
	/** XP earned since the app started. */
	session: number;
	/** Account total as last reported by the server (xpTotal), if known. */
	total?: number;
	submissions: number;
	validated: number;
}
