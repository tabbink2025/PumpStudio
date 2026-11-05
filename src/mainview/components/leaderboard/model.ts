import { useEffect, useRef, useState } from "react";
import { extractMint } from "../../../shared/analysis";
import type { AnalysisStats, LeaderboardEntry } from "../../../shared/types";
import { bun, errorMessage } from "../../lib/rpc";
import { useApp } from "../../lib/store";

/** How many tokens the leaderboard requests. */
export const LEADERBOARD_LIMIT = 50;
/** Data older than this is refetched when the screen becomes active again. */
const STALE_MS = 2 * 60_000;

export interface RankedEntry extends LeaderboardEntry {
	/** 1-based rank by analysis count, as returned by the server. */
	rank: number;
}

export type SortKey = "rank" | "count" | "score";
export type SortDir = "asc" | "desc";
export interface Sort {
	key: SortKey;
	dir: SortDir;
}

export const DEFAULT_SORT: Sort = { key: "rank", dir: "asc" };

/** First click on a column sorts in its natural direction. */
export function nextSort(cur: Sort, key: SortKey): Sort {
	if (cur.key === key) return { key, dir: cur.dir === "asc" ? "desc" : "asc" };
	return { key, dir: key === "rank" ? "asc" : "desc" };
}

export function sortEntries(list: RankedEntry[], sort: Sort): RankedEntry[] {
	const sign = sort.dir === "asc" ? 1 : -1;
	const out = [...list];
	out.sort((a, b) => {
		let d = 0;
		if (sort.key === "rank") d = a.rank - b.rank;
		else if (sort.key === "count") d = a.count - b.count || b.rank - a.rank;
		else {
			// Missing scores always sink to the bottom.
			const as = a.lastScore ?? null;
			const bs = b.lastScore ?? null;
			if (as == null && bs == null) return a.rank - b.rank;
			if (as == null) return 1;
			if (bs == null) return -1;
			d = as - bs || b.rank - a.rank;
		}
		return d * sign;
	});
	return out;
}

/** Match on ticker (with or without $), name or mint (also accepts pasted links). */
export function filterEntries(list: RankedEntry[], query: string): RankedEntry[] {
	const q = query.trim().toLowerCase().replace(/^\$/, "");
	if (!q) return list;
	const mint = extractMint(query);
	return list.filter((e) => {
		if (mint && e.mint === mint) return true;
		return (
			(e.symbol ?? "").toLowerCase().includes(q) ||
			(e.name ?? "").toLowerCase().includes(q) ||
			e.mint.toLowerCase().startsWith(q)
		);
	});
}

export function scoreTone(score: number): "pos" | "warn" | "neg" {
	if (score >= 65) return "pos";
	if (score <= 35) return "neg";
	return "warn";
}

/* ---- Data hook ----------------------------------------------------------- */

export interface StatsState {
	stats: AnalysisStats | null;
	loading: boolean;
	error?: string;
	fetchedAt?: number;
	reload: () => void;
}

/**
 * Loads the global analysis stats. Fetches on first activation, again when the
 * data went stale while the screen was hidden, and whenever the global refresh
 * command bumps refreshTick while this screen is active.
 */
export function useAnalysisStats(active: boolean): StatsState {
	const [stats, setStats] = useState<AnalysisStats | null>(null);
	const [loading, setLoading] = useState(false);
	const [error, setError] = useState<string>();
	const [fetchedAt, setFetchedAt] = useState<number>();
	const reqId = useRef(0);
	const fetchedAtRef = useRef<number>();
	const started = useRef(false);

	const load = useRef(async () => {
		const id = ++reqId.current;
		started.current = true;
		setLoading(true);
		try {
			const s = await bun.getAnalysisStats(LEADERBOARD_LIMIT);
			if (id !== reqId.current) return;
			setStats(s);
			setError(undefined);
			const now = Date.now();
			fetchedAtRef.current = now;
			setFetchedAt(now);
			useApp
				.getState()
				.rememberTokens(s.leaderboard.map((e) => ({ mint: e.mint, symbol: e.symbol, name: e.name, image: e.image })));
		} catch (err) {
			if (id !== reqId.current) return;
			setError(errorMessage(err));
		} finally {
			if (id === reqId.current) setLoading(false);
		}
	}).current;

	// First activation, and refetch stale data when coming back to the screen.
	useEffect(() => {
		if (!active) return;
		if (!started.current) void load();
		else if (fetchedAtRef.current && Date.now() - fetchedAtRef.current > STALE_MS) void load();
	}, [active, load]);

	// Global refresh (Cmd/Ctrl+R, menu) while this screen is visible.
	const tick = useApp((s) => s.refreshTick);
	const lastTick = useRef(tick);
	useEffect(() => {
		if (tick === lastTick.current) return;
		lastTick.current = tick;
		if (active) void load();
	}, [tick, active, load]);

	return { stats, loading, error, fetchedAt, reload: () => void load() };
}

/** Re-render every `ms` while enabled, for relative timestamps. */
export function useNow(enabled: boolean, ms = 15_000): number {
	const [now, setNow] = useState(() => Date.now());
	useEffect(() => {
		if (!enabled) return;
		setNow(Date.now());
		const t = setInterval(() => setNow(Date.now()), ms);
		return () => clearInterval(t);
	}, [enabled, ms]);
	return now;
}

/** Width of an element, for column tiers that follow the container, not the window. */
export function useWidth<T extends HTMLElement>(): [(el: T | null) => void, number] {
	const [el, setEl] = useState<T | null>(null);
	const [w, setW] = useState(0);
	useEffect(() => {
		if (!el) return;
		const ro = new ResizeObserver((entries) => {
			const next = Math.round(entries[0]?.contentRect.width ?? 0);
			setW((prev) => (prev === next ? prev : next));
		});
		ro.observe(el);
		return () => ro.disconnect();
	}, [el]);
	return [setEl, w];
}
