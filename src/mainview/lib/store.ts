/* ------------------------------------------------------------------ */
/*  App state (zustand). Lives above the screens, so switching views    */
/*  never loses a draft, a filter or a running training session.        */
/* ------------------------------------------------------------------ */

import { create } from "zustand";
import type { ViewId } from "../../shared/rpc-schema";
import type {
	ConfigPatch,
	MarketTab,
	MarketToken,
	PublicConfig,
	Sentiment,
	TrainingState,
	UiPrefs,
	XpState,
} from "../../shared/types";
import { bun, errorMessage } from "./rpc";

/* ---- Toasts -------------------------------------------------------- */

export type ToastKind = "success" | "error" | "info" | "xp";

export interface Toast {
	id: number;
	kind: ToastKind;
	title: string;
	description?: string;
	action?: { label: string; run: () => void };
	/** ms; 0 = sticky until dismissed. Errors default to sticky. */
	duration: number;
}

/* ---- Market cache --------------------------------------------------- */

export interface MarketSlice {
	tokens: MarketToken[];
	fetchedAt?: number;
	loading: boolean;
	error?: string;
}

/** Tokens the user analyzed (this device), for "analyzed" badges and filters. */
export interface MyAnalysis {
	sentiment: Sentiment;
	score: number;
	at: number;
	xp?: number;
	validated?: boolean;
	count: number;
}

export interface TokenMeta {
	symbol?: string;
	name?: string;
	image?: string;
}

const ANALYZED_KEY = "pumpstudio.analyzed.v1";

function loadAnalyzed(): Record<string, MyAnalysis> {
	try {
		const raw = localStorage.getItem(ANALYZED_KEY);
		return raw ? (JSON.parse(raw) as Record<string, MyAnalysis>) : {};
	} catch {
		return {};
	}
}

function saveAnalyzed(map: Record<string, MyAnalysis>) {
	try {
		const entries = Object.entries(map)
			.sort((a, b) => b[1].at - a[1].at)
			.slice(0, 1000);
		localStorage.setItem(ANALYZED_KEY, JSON.stringify(Object.fromEntries(entries)));
	} catch {}
}

const IDLE_TRAINING: TrainingState = {
	status: "idle",
	runId: 0,
	total: 0,
	completed: 0,
	failed: 0,
	skipped: 0,
	xpEarned: 0,
	validatedCount: 0,
	intervalSec: 10,
	orchestrator: "manual",
	items: [],
};

interface AppState {
	view: ViewId;
	/** Mint currently open in the Analyze screen. */
	analyzeMint: string | null;
	/** Mint open in the Tokens inspector panel. */
	inspectMint: string | null;
	paletteOpen: boolean;
	shortcutsOpen: boolean;
	/** Incremented by the global "refresh" command; screens refetch when it changes. */
	refreshTick: number;

	config: PublicConfig | null;
	configError?: string;
	xp: XpState;
	/** Last XP gain, for the animated "+25" in the sidebar. */
	xpFlash?: { amount: number; at: number };
	training: TrainingState;

	market: Partial<Record<MarketTab, MarketSlice>>;
	tokenMeta: Record<string, TokenMeta>;
	analyzed: Record<string, MyAnalysis>;

	toasts: Toast[];

	setView: (v: ViewId) => void;
	openAnalyze: (mint: string | null) => void;
	setInspect: (mint: string | null) => void;
	setPalette: (open: boolean) => void;
	setShortcuts: (open: boolean) => void;
	refresh: () => void;

	loadConfig: () => Promise<void>;
	saveConfig: (patch: ConfigPatch, opts?: { silent?: boolean }) => Promise<PublicConfig | null>;
	setPrefs: (prefs: Partial<UiPrefs>) => void;
	toggleWatch: (mint: string) => void;
	pushRecent: (mint: string) => void;

	setXp: (xp: XpState) => void;
	setTraining: (t: TrainingState) => void;

	fetchMarket: (tab: MarketTab, opts?: { force?: boolean }) => Promise<void>;
	rememberTokens: (tokens: Array<{ mint: string } & TokenMeta>) => void;
	recordAnalysis: (mint: string, a: Omit<MyAnalysis, "count" | "at">) => void;

	toast: (t: Omit<Toast, "id" | "duration"> & { duration?: number }) => number;
	dismissToast: (id: number) => void;
}

let toastId = 0;
const MARKET_TTL_MS = 30_000;

export const useApp = create<AppState>((set, get) => ({
	view: "tokens",
	analyzeMint: null,
	inspectMint: null,
	paletteOpen: false,
	shortcutsOpen: false,
	refreshTick: 0,

	config: null,
	xp: { session: 0, submissions: 0, validated: 0 },
	training: IDLE_TRAINING,

	market: {},
	tokenMeta: {},
	analyzed: loadAnalyzed(),

	toasts: [],

	setView: (view) => set({ view, paletteOpen: false }),
	openAnalyze: (mint) => {
		set({ view: "analyze", analyzeMint: mint, paletteOpen: false });
		if (mint) get().pushRecent(mint);
	},
	setInspect: (inspectMint) => set({ inspectMint }),
	setPalette: (paletteOpen) => set({ paletteOpen }),
	setShortcuts: (shortcutsOpen) => set({ shortcutsOpen }),
	refresh: () => set((s) => ({ refreshTick: s.refreshTick + 1 })),

	loadConfig: async () => {
		try {
			const config = await bun.loadConfig();
			set({ config, configError: undefined });
		} catch (err) {
			set({ configError: errorMessage(err) });
		}
	},

	saveConfig: async (patch, opts) => {
		try {
			const config = await bun.saveConfig(patch);
			set({ config });
			if (!opts?.silent) get().toast({ kind: "success", title: "Settings saved" });
			return config;
		} catch (err) {
			get().toast({ kind: "error", title: "Could not save settings", description: errorMessage(err) });
			return null;
		}
	},

	setPrefs: (prefs) => {
		const cfg = get().config;
		if (cfg) set({ config: { ...cfg, prefs: { ...cfg.prefs, ...prefs } } }); // optimistic
		void get().saveConfig({ prefs }, { silent: true });
	},

	toggleWatch: (mint) => {
		const cfg = get().config;
		if (!cfg) return;
		const list = cfg.prefs.watchlist;
		const watching = list.includes(mint);
		const next = watching ? list.filter((m) => m !== mint) : [mint, ...list];
		get().setPrefs({ watchlist: next });
		const meta = get().tokenMeta[mint];
		get().toast({
			kind: "info",
			title: watching ? "Removed from watchlist" : "Added to watchlist",
			description: meta?.symbol ? `$${meta.symbol}` : undefined,
			action: { label: "Undo", run: () => get().setPrefs({ watchlist: list }) },
			duration: 4000,
		});
	},

	pushRecent: (mint) => {
		const cfg = get().config;
		if (!cfg) return;
		const next = [mint, ...cfg.prefs.recentMints.filter((m) => m !== mint)].slice(0, 10);
		get().setPrefs({ recentMints: next });
	},

	setXp: (xp) => {
		const prev = get().xp;
		const gained = xp.session - prev.session;
		set({ xp, ...(gained > 0 ? { xpFlash: { amount: gained, at: Date.now() } } : {}) });
	},

	setTraining: (training) => {
		// Fold finished training items into "analyzed by me".
		const analyzed = { ...get().analyzed };
		let changed = false;
		for (const it of training.items) {
			if (it.step === "done" && it.sentiment && it.score != null) {
				const prev = analyzed[it.mint];
				if (!prev || prev.at < it.updatedAt) {
					analyzed[it.mint] = {
						sentiment: it.sentiment,
						score: it.score,
						at: it.updatedAt,
						xp: it.xpEarned,
						validated: it.validated,
						count: (prev?.count ?? 0) + 1,
					};
					changed = true;
				}
			}
		}
		if (changed) saveAnalyzed(analyzed);
		set({ training, ...(changed ? { analyzed } : {}) });
		get().rememberTokens(training.items);
	},

	fetchMarket: async (tab, opts) => {
		const slice = get().market[tab];
		if (slice?.loading) return;
		if (!opts?.force && slice?.fetchedAt && Date.now() - slice.fetchedAt < MARKET_TTL_MS) return;
		set((s) => ({ market: { ...s.market, [tab]: { tokens: slice?.tokens ?? [], fetchedAt: slice?.fetchedAt, loading: true } } }));
		try {
			const tokens = await bun.getMarket({ tab, limit: 50 });
			set((s) => ({ market: { ...s.market, [tab]: { tokens, fetchedAt: Date.now(), loading: false } } }));
			get().rememberTokens(tokens.map((t) => ({ mint: t.mint, symbol: t.symbol, name: t.name, image: t.image_uri })));
		} catch (err) {
			set((s) => ({
				market: {
					...s.market,
					[tab]: { tokens: slice?.tokens ?? [], fetchedAt: slice?.fetchedAt, loading: false, error: errorMessage(err) },
				},
			}));
		}
	},

	rememberTokens: (tokens) => {
		const meta = get().tokenMeta;
		let changed = false;
		const next = { ...meta };
		for (const t of tokens) {
			if (!t?.mint) continue;
			const cur = next[t.mint];
			const merged = {
				symbol: t.symbol ?? cur?.symbol,
				name: t.name ?? cur?.name,
				image: t.image ?? cur?.image,
			};
			if (!cur || cur.symbol !== merged.symbol || cur.name !== merged.name || cur.image !== merged.image) {
				next[t.mint] = merged;
				changed = true;
			}
		}
		if (changed) set({ tokenMeta: next });
	},

	recordAnalysis: (mint, a) => {
		const prev = get().analyzed[mint];
		const analyzed = { ...get().analyzed, [mint]: { ...a, at: Date.now(), count: (prev?.count ?? 0) + 1 } };
		saveAnalyzed(analyzed);
		set({ analyzed });
	},

	toast: (t) => {
		const id = ++toastId;
		const duration = t.duration ?? (t.kind === "error" ? 0 : 5000);
		set((s) => ({ toasts: [...s.toasts.slice(-2), { ...t, id, duration }] }));
		return id;
	},
	dismissToast: (id) => set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })),
}));

/* ---- Selectors ----------------------------------------------------- */

export const useConfig = () => useApp((s) => s.config);
export const usePrefs = () => useApp((s) => s.config?.prefs);
export const toast = (t: Parameters<AppState["toast"]>[0]) => useApp.getState().toast(t);
