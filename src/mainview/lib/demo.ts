/* ------------------------------------------------------------------ */
/*  Demo backend: used when the UI runs outside the desktop app.        */
/*  Deterministic, generated data (fictional tokens), no network.       */
/* ------------------------------------------------------------------ */

import { computeDefaults } from "../../shared/analysis";
import type { BunRequests, WebviewMessages } from "../../shared/rpc-schema";
import type {
	AnalysisStats,
	DataPoint,
	MarketTab,
	MarketToken,
	PublicConfig,
	TrainingItem,
	TrainingState,
	XpState,
} from "../../shared/types";

type Emit = <K extends keyof WebviewMessages>(name: K, payload: WebviewMessages[K]) => void;

const B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

function rng(seed: number) {
	let s = seed >>> 0 || 1;
	return () => {
		s ^= s << 13;
		s ^= s >>> 17;
		s ^= s << 5;
		return ((s >>> 0) % 1_000_000) / 1_000_000;
	};
}

function hash(str: string): number {
	let h = 2166136261;
	for (let i = 0; i < str.length; i++) h = Math.imul(h ^ str.charCodeAt(i), 16777619);
	return h >>> 0;
}

const WORDS = [
	"Moon", "Frog", "Cat", "Pixel", "Turbo", "Lofi", "Bonk", "Nebula", "Mochi", "Glitch", "Sushi", "Comet",
	"Panda", "Orbit", "Noodle", "Rocket", "Wizard", "Toast", "Banana", "Quasar", "Pickle", "Zebra", "Cloud",
	"Mango", "Ghost", "Disco", "Waffle", "Cosmo", "Pepper", "Yeti", "Kiwi", "Taco", "Laser", "Otter", "Fizz",
	"Bubble", "Llama", "Neon", "Puddle", "Saturn", "Biscuit", "Gecko", "Hamster", "Jelly", "Koala", "Maple",
];
const SUFFIX = ["Inu", "Coin", "AI", "Cat", "Dog", "Club", "Mode", "Wif Hat", "Classic", "World", "Party", "Labs", ""];

function makeMint(r: () => number): string {
	let s = "";
	for (let i = 0; i < 40; i++) s += B58[Math.floor(r() * B58.length)];
	return `${s}pump`;
}

function makeToken(seed: number, tab: MarketTab, now: number): MarketToken {
	const r = rng(seed);
	const a = WORDS[Math.floor(r() * WORDS.length)]!;
	const b = SUFFIX[Math.floor(r() * SUFFIX.length)]!;
	const name = b ? `${a} ${b}` : a;
	const symbol = (a + (b ? b.replace(/\s/g, "").slice(0, 2) : "")).toUpperCase().slice(0, 8);
	const graduated = tab === "graduated" || (tab === "all" && r() < 0.18);
	const ageMin = tab === "new" ? r() * 45 : graduated ? 60 * (2 + r() * 70) : 5 + r() * 600;
	const mcap = graduated ? 70_000 + r() ** 2 * 8_000_000 : 3_000 + r() ** 2.4 * 65_000;
	return {
		mint: makeMint(r),
		name,
		symbol,
		usd_market_cap: Math.round(mcap),
		created_timestamp: Math.round(now - ageMin * 60_000),
		is_currently_live: tab === "live" ? true : r() < 0.12,
		complete: graduated,
	};
}

function makeDataPoint(t: MarketToken, now: number): DataPoint {
	const r = rng(hash(t.mint));
	const mcap = t.usd_market_cap ?? 10_000;
	const graduated = !!t.complete;
	const holders = Math.round(graduated ? 400 + r() * 6000 : 15 + r() * 900);
	const buys = Math.round(20 + r() * (graduated ? 4000 : 900));
	const sells = Math.round(buys * (0.35 + r() * 1.3));
	const top10 = graduated ? 12 + r() * 35 : 22 + r() * 62;
	const hasX = r() < 0.6;
	const supply = 1_000_000_000;
	return {
		mint: t.mint,
		name: t.name,
		symbol: t.symbol,
		description: r() < 0.7 ? `${t.name} is a community token on pump.fun. Demo data, not a real token.` : undefined,
		priceUsd: mcap / supply,
		priceSol: mcap / supply / 160,
		solPriceUsd: 160,
		priceChange1h: (r() - 0.45) * 40,
		priceChange6h: (r() - 0.45) * 80,
		priceChange24h: (r() - 0.4) * 160,
		marketCap: mcap,
		fdv: mcap,
		totalSupply: supply,
		liquidity: graduated ? mcap * (0.08 + r() * 0.2) : mcap * (0.15 + r() * 0.25),
		volume1h: mcap * r() * 0.08,
		volume24h: mcap * (0.02 + r() * 1.1),
		buys24h: buys,
		sells24h: sells,
		tradeCount: buys + sells,
		holderCount: holders,
		bondingComplete: graduated,
		bondingProgress: graduated ? 100 : Math.min(99.5, (mcap / 69_000) * 100),
		isLive: !!t.is_currently_live,
		streamSource: t.is_currently_live ? "pumpfun" : "none",
		viewerCount: t.is_currently_live ? Math.round(r() * 900) : null,
		website: r() < 0.35 ? "https://example.com" : null,
		twitter: hasX ? "https://x.com/example" : null,
		telegram: r() < 0.3 ? "https://t.me/example" : null,
		top10Holding: top10,
		creatorHolding: r() * 12,
		snipersHolding: r() * 25,
		insidersHolding: r() * 18,
		topHolders: Array.from({ length: 10 }, (_, i) => ({
			address: makeMint(rng(hash(t.mint) + i + 1)).slice(0, 44),
			amount: 0,
			pct: (top10 / 10) * (1.6 - i * 0.12),
		})),
		snapshotAt: now,
		ttl: 30_000,
		version: "demo",
	};
}

const STORE_KEY = "pumpstudio-demo-config";

function loadDemoConfig(): PublicConfig {
	const base: PublicConfig = {
		apiKeySet: true,
		apiKeyHint: "ps_…d3m0",
		orchestrator: "claude",
		claudeKeySet: true,
		claudeKeyHint: "sk-…demo",
		openaiKeySet: false,
		openaiKeyHint: "",
		claudeModel: "claude-opus-5-5",
		openaiModel: "gpt-5.5",
		analyzeInterval: 10,
		skipRecentlyAnalyzed: true,
		prefs: { theme: "system", density: "comfortable", sidebarCollapsed: false, watchlist: [], recentMints: [] },
		configPath: "~/.pump-studio/trainer.json",
	};
	try {
		const raw = localStorage.getItem(STORE_KEY);
		if (raw) return { ...base, ...JSON.parse(raw), prefs: { ...base.prefs, ...JSON.parse(raw).prefs } };
	} catch {}
	return base;
}

const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

export function createDemoBackend(emit: Emit) {
	const now = Date.now();
	const tabs: MarketTab[] = ["all", "live", "new", "graduated"];
	const market = new Map<MarketTab, MarketToken[]>();
	tabs.forEach((tab, ti) => {
		market.set(
			tab,
			Array.from({ length: 50 }, (_, i) => makeToken(9973 * (ti + 1) + i * 7919, tab, now)).sort(
				(a, b) => (b.usd_market_cap ?? 0) - (a.usd_market_cap ?? 0),
			),
		);
	});
	const byMint = new Map<string, MarketToken>();
	market.forEach((list) => list.forEach((t) => byMint.set(t.mint, t)));

	let config = loadDemoConfig();
	const xp: XpState = { session: 0, total: 12_480, submissions: 0, validated: 0 };
	const analyzed = new Map<string, { count: number; sentiment: string; score: number }>();
	let training: TrainingState = {
		status: "idle",
		runId: 0,
		total: 0,
		completed: 0,
		failed: 0,
		skipped: 0,
		xpEarned: 0,
		validatedCount: 0,
		intervalSec: config.analyzeInterval,
		orchestrator: config.orchestrator,
		items: [],
	};

	const dp = (mint: string): DataPoint => {
		const t = byMint.get(mint) ?? makeToken(hash(mint), "all", now);
		if (!byMint.has(mint)) byMint.set(mint, { ...t, mint });
		return makeDataPoint({ ...t, mint }, Date.now());
	};

	const submit = (validated: boolean) => {
		const earned = validated ? 25 : 5;
		xp.session += earned;
		xp.total = (xp.total ?? 0) + earned;
		xp.submissions++;
		if (validated) xp.validated++;
		emit("xp", { ...xp });
		return earned;
	};

	const pushTraining = () => emit("trainingState", structuredClone(training));

	async function runTraining(runId: number) {
		for (let i = 0; i < training.items.length; i++) {
			if (training.runId !== runId || training.status !== "running") break;
			const set = (patch: Partial<TrainingItem>) => {
				training.items[i] = { ...training.items[i]!, ...patch, updatedAt: Date.now() };
				pushTraining();
			};
			set({ step: "fetching" });
			await wait(500);
			const d = dp(training.items[i]!.mint);
			set({ step: "analyzing", symbol: d.symbol, name: d.name });
			await wait(900);
			const a = computeDefaults(d);
			set({ step: "submitting", sentiment: a.sentiment, score: a.score, source: config.orchestrator === "manual" ? "heuristic" : config.orchestrator });
			await wait(500);
			if (rng(hash(d.mint))() < 0.08) {
				training.failed++;
				set({ step: "error", failedStep: "submitting", error: "Rate limited by pump.studio. Wait a moment and try again." });
			} else {
				const validated = rng(hash(d.mint) + 3)() > 0.15;
				const earned = submit(validated);
				training.completed++;
				training.xpEarned += earned;
				if (validated) training.validatedCount++;
				set({ step: "done", xpEarned: earned, validated, deviationPct: validated ? 2 + rng(hash(d.mint))() * 8 : 19.4 });
			}
			if (i < training.items.length - 1 && training.status === "running") {
				training.nextAt = Date.now() + 2500;
				pushTraining();
				await wait(2500);
				training.nextAt = undefined;
			}
		}
		if (training.runId !== runId) return;
		const stopped = training.status === "stopping";
		training.items = training.items.map((it) => (it.step === "queued" ? { ...it, step: "skipped", error: stopped ? "Stopped" : undefined } : it));
		training.skipped = training.items.filter((it) => it.step === "skipped").length;
		training.status = "idle";
		training.finishedAt = Date.now();
		pushTraining();
	}

	async function request<M extends keyof BunRequests>(method: M, params: BunRequests[M]["params"]): Promise<unknown> {
		const p = params as never as Record<string, unknown>;
		switch (method) {
			case "getMarket": {
				await wait(350);
				return market.get((p.tab as MarketTab) ?? "all")!.slice(0, (p.limit as number) ?? 50);
			}
			case "getDataPoint": {
				await wait(300);
				const mint = String(p.mint).trim();
				if (!/^[1-9A-HJ-NP-Za-km-z]{32,44}$/.test(mint)) throw new Error("That does not look like a Solana mint address");
				return dp(mint);
			}
			case "getAnalysisStats": {
				await wait(400);
				const entries = [...byMint.values()].slice(0, 40).map((t, i) => {
					const r = rng(hash(t.mint) + 11);
					const own = analyzed.get(t.mint);
					const score = own?.score ?? Math.round(20 + r() * 70);
					return {
						mint: t.mint,
						name: t.name,
						symbol: t.symbol,
						count: Math.round(400 / (i + 1) + r() * 30) + (own?.count ?? 0),
						lastSentiment: own?.sentiment ?? (score > 65 ? "bullish" : score < 35 ? "bearish" : "neutral"),
						lastScore: score,
					};
				});
				entries.sort((a, b) => b.count - a.count);
				const stats: AnalysisStats = { totalAnalyses: 27_841 + xp.submissions, uniqueTokens: 6_213, goal: 100_000, progress: 27.8, leaderboard: entries.slice(0, (p.limit as number) ?? 25) };
				return stats;
			}
			case "submitAnalysis": {
				await wait(700);
				const payload = params as BunRequests["submitAnalysis"]["params"];
				const validated = rng(hash(payload.mint) + Date.now())() > 0.2;
				const earned = submit(validated);
				const prev = analyzed.get(payload.mint);
				analyzed.set(payload.mint, { count: (prev?.count ?? 0) + 1, sentiment: payload.sentiment, score: payload.score });
				return {
					ok: true,
					xpEarned: earned,
					xpTotal: xp.total,
					analysisId: `demo_${Date.now().toString(36)}`,
					validated,
					deviationPct: validated ? 3.2 : 18.7,
					warning: validated ? undefined : "Snapshot differs from live data by more than 15%. Refresh the token and resubmit.",
				};
			}
			case "orchestrateAnalysis": {
				await wait(1600);
				const d = (p.dataPoint as DataPoint) ?? dp(String(p.mint));
				const a = computeDefaults(d);
				const ai = config.orchestrator !== "manual";
				return {
					...a,
					score: ai ? Math.max(0, Math.min(100, a.score + 4)) : a.score,
					summary: ai
						? `${d.symbol} shows ${a.buyPressure}% buy pressure with ${a.liquidityDepth} liquidity and ${a.holderConcentration.replace("_", " ")} holders. ${a.riskLevel === "low" ? "Risk looks contained" : `Risk is ${a.riskLevel}`}; volume is ${a.volumeProfile} relative to market cap.`
						: a.summary,
					source: ai ? config.orchestrator : "heuristic",
					model: ai ? (config.orchestrator === "claude" ? config.claudeModel : config.openaiModel) : undefined,
					durationMs: 1600,
				};
			}
			case "loadConfig":
				await wait(50);
				return config;
			case "saveConfig": {
				await wait(150);
				const patch = params as BunRequests["saveConfig"]["params"];
				const key = (v: string | null | undefined, set: boolean, hint: string) =>
					v === undefined ? { set, hint } : v ? { set: true, hint: `${v.slice(0, 3)}…${v.slice(-4)}` } : { set: false, hint: "" };
				const k1 = key(patch.apiKey, config.apiKeySet, config.apiKeyHint);
				const k2 = key(patch.claudeApiKey, config.claudeKeySet, config.claudeKeyHint);
				const k3 = key(patch.openaiApiKey, config.openaiKeySet, config.openaiKeyHint);
				config = {
					...config,
					apiKeySet: k1.set,
					apiKeyHint: k1.hint,
					claudeKeySet: k2.set,
					claudeKeyHint: k2.hint,
					openaiKeySet: k3.set,
					openaiKeyHint: k3.hint,
					orchestrator: patch.orchestrator ?? config.orchestrator,
					claudeModel: patch.claudeModel ?? config.claudeModel,
					openaiModel: patch.openaiModel ?? config.openaiModel,
					analyzeInterval: patch.analyzeInterval != null ? Math.max(6, patch.analyzeInterval) : config.analyzeInterval,
					skipRecentlyAnalyzed: patch.skipRecentlyAnalyzed ?? config.skipRecentlyAnalyzed,
					prefs: { ...config.prefs, ...(patch.prefs ?? {}) },
				};
				try {
					localStorage.setItem(STORE_KEY, JSON.stringify(config));
				} catch {}
				return config;
			}
			case "checkApiKey": {
				await wait(600);
				const k = (p.apiKey as string | undefined) ?? "";
				if (!k && !config.apiKeySet) return { status: "missing" };
				if (k && !k.startsWith("ps_")) return { status: "invalid", message: "pump.studio keys start with ps_" };
				return { status: "valid", agentName: "demo-agent" };
			}
			case "registerApiKey":
				await wait(800);
				return { key: `ps_demo${Math.random().toString(36).slice(2, 14)}`, type: "agent", rateLimit: 300 };
			case "getTrainingState":
				return structuredClone(training);
			case "startAutoTraining": {
				const targets = (params as BunRequests["startAutoTraining"]["params"]).targets;
				if (training.status !== "idle") throw new Error("A training run is already active");
				if (!config.apiKeySet) throw new Error("Add your pump.studio API key in Settings first");
				if (!targets.length) throw new Error("Select at least one token");
				training = {
					status: "running",
					runId: training.runId + 1,
					startedAt: Date.now(),
					total: targets.length,
					completed: 0,
					failed: 0,
					skipped: 0,
					xpEarned: 0,
					validatedCount: 0,
					intervalSec: config.analyzeInterval,
					orchestrator: config.orchestrator,
					items: targets.map((t) => ({ ...t, step: "queued", updatedAt: Date.now() })),
				};
				pushTraining();
				void runTraining(training.runId);
				return structuredClone(training);
			}
			case "stopAutoTraining":
				if (training.status === "running") {
					training.status = "stopping";
					pushTraining();
				}
				return structuredClone(training);
			case "getXp":
				return { ...xp };
			case "openExternal":
			case "openWebApp":
				return true;
		}
		throw new Error(`Unknown request ${String(method)}`);
	}

	return { request };
}
