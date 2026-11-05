/* ------------------------------------------------------------------ */
/*  Tokens screen: list model (tabs, filters, sorting) and small        */
/*  helpers shared by the table, toolbar and inspector.                 */
/* ------------------------------------------------------------------ */

import { shortMint } from "../../../shared/analysis";
import type { MarketTab, MarketToken } from "../../../shared/types";
import type { MarketSlice, MyAnalysis, TokenMeta } from "../../lib/store";
import { toast } from "../../lib/store";

export type TokensTab = MarketTab | "watchlist";

export const MARKET_TABS: readonly MarketTab[] = ["all", "live", "new", "graduated"];

export const TAB_LABEL: Record<TokensTab, string> = {
	all: "All",
	live: "Live",
	new: "New",
	graduated: "Graduated",
	watchlist: "Watchlist",
};

export type FilterId = "notAnalyzed" | "live" | "onCurve" | "young";

export const FILTERS: ReadonlyArray<{ id: FilterId; label: string; title: string }> = [
	{ id: "notAnalyzed", label: "Not analyzed by me", title: "Not analyzed by me: hide tokens you already analyzed on this device" },
	{ id: "live", label: "Live now", title: "Only tokens that are livestreaming right now" },
	{ id: "onCurve", label: "On curve", title: "Only tokens still on the bonding curve (not graduated)" },
	{ id: "young", label: "Under 1h old", title: "Only tokens created in the last hour" },
];

export type SortKey = "name" | "age" | "mcap";
export type SortDir = "asc" | "desc";
export interface Sort {
	key: SortKey;
	dir: SortDir;
}

export const DEFAULT_SORT: Sort = { key: "mcap", dir: "desc" };

/** First click on a column uses the most useful direction for it. */
export const FIRST_DIR: Record<SortKey, SortDir> = { name: "asc", age: "asc", mcap: "desc" };

export const EMPTY_TOKENS: MarketToken[] = [];
export const EMPTY_LIST: string[] = [];

export function marketCap(t: MarketToken): number | undefined {
	const v = t.usd_market_cap ?? undefined;
	return typeof v === "number" && Number.isFinite(v) ? v : undefined;
}

/**
 * Watchlist rows: every watched mint, in watchlist order. Metrics come from
 * whichever cached market slice has the token (most recently fetched wins);
 * unknown mints fall back to remembered metadata with empty metrics.
 */
export function watchlistTokens(
	watchlist: readonly string[],
	market: Partial<Record<MarketTab, MarketSlice>>,
	meta: Record<string, TokenMeta>,
): MarketToken[] {
	if (watchlist.length === 0) return EMPTY_TOKENS;
	const slices = MARKET_TABS.map((t) => market[t])
		.filter((s): s is MarketSlice => !!s)
		.sort((a, b) => (a.fetchedAt ?? 0) - (b.fetchedAt ?? 0));
	const byMint = new Map<string, MarketToken>();
	for (const s of slices) for (const t of s.tokens) byMint.set(t.mint, t);
	return watchlist.map((mint) => {
		const t = byMint.get(mint);
		if (t) return t;
		const m = meta[mint];
		return { mint, name: m?.name ?? "", symbol: m?.symbol ?? "", image_uri: m?.image };
	});
}

export function applyFilters(
	tokens: readonly MarketToken[],
	filters: ReadonlySet<FilterId>,
	query: string,
	analyzed: Record<string, MyAnalysis>,
	now: number,
): MarketToken[] {
	const q = query.trim().toLowerCase().replace(/^\$/, "");
	return tokens.filter((t) => {
		if (filters.has("notAnalyzed") && analyzed[t.mint]) return false;
		if (filters.has("live") && !t.is_currently_live) return false;
		if (filters.has("onCurve") && t.complete === true) return false;
		if (filters.has("young")) {
			const c = t.created_timestamp;
			if (!c || now - c > 3_600_000) return false;
		}
		if (q) {
			const hay = `${t.symbol ?? ""}\n${t.name ?? ""}\n${t.mint}`.toLowerCase();
			if (!hay.includes(q)) return false;
		}
		return true;
	});
}

export function sortTokens(tokens: MarketToken[], sort: Sort): MarketToken[] {
	const mul = sort.dir === "asc" ? 1 : -1;
	const out = tokens.slice();
	out.sort((a, b) => {
		if (sort.key === "name") {
			const an = (a.symbol || a.name || "").toLowerCase();
			const bn = (b.symbol || b.name || "").toLowerCase();
			if (!an !== !bn) return an ? -1 : 1; // unknown names last
			return an.localeCompare(bn) * mul;
		}
		if (sort.key === "age") {
			// Ascending age = youngest first = newest created_timestamp first.
			const av = a.created_timestamp;
			const bv = b.created_timestamp;
			if (av == null || bv == null) return av == null ? (bv == null ? 0 : 1) : -1;
			return (bv - av) * mul;
		}
		const av = marketCap(a);
		const bv = marketCap(b);
		if (av == null || bv == null) return av == null ? (bv == null ? 0 : 1) : -1;
		return (av - bv) * mul;
	});
	return out;
}

/** "$SYM" or a short mint when the token has no known symbol. */
export function tokenLabel(t: { mint: string; symbol?: string }): string {
	return t.symbol ? `$${t.symbol}` : shortMint(t.mint);
}

export async function copyMint(mint: string) {
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
	toast({ kind: "success", title: "Mint address copied", description: shortMint(mint, 6, 6), duration: 2500 });
}

/* ---- Per-viewer conveniences ----------------------------------------- */

export type AutoRefresh = "off" | "30" | "60";
const AUTO_KEY = "pumpstudio.tokens.autoRefresh.v1";

export function loadAutoRefresh(): AutoRefresh {
	try {
		const v = localStorage.getItem(AUTO_KEY);
		return v === "30" || v === "60" ? v : "off";
	} catch {
		return "off";
	}
}

export function saveAutoRefresh(v: AutoRefresh) {
	try {
		localStorage.setItem(AUTO_KEY, v);
	} catch {}
}

/** Normalize creator-provided social links into URLs. */
export function socialUrl(kind: "website" | "twitter" | "telegram", raw: string | null | undefined): string | null {
	const v = (raw ?? "").trim();
	if (!v) return null;
	if (/^https?:\/\//i.test(v)) return v;
	if (kind === "twitter") return `https://x.com/${v.replace(/^@/, "")}`;
	if (kind === "telegram") return `https://t.me/${v.replace(/^@/, "")}`;
	return `https://${v}`;
}
