import type {
	AnalysisPayload,
	AnalysisStats,
	DataPoint,
	KeyCheck,
	LeaderboardEntry,
	MarketTab,
	MarketToken,
	RegisteredKey,
	SubmitResult,
} from "../shared/types";

/* ------------------------------------------------------------------ */
/*  Pump Studio HTTP client                                             */
/*                                                                      */
/*  Matches the official clients: every response is { ok, data, error } */
/*  and errors carry a JSON { error } body.                             */
/* ------------------------------------------------------------------ */

const BASE = (process.env.PUMP_API_BASE || "https://api.pump.studio").replace(/\/$/, "");
const TIMEOUT_MS = 20_000;

export class ApiError extends Error {
	constructor(
		message: string,
		readonly status: number,
	) {
		super(message);
		this.name = "ApiError";
	}
}

type Envelope<T> = { ok?: boolean; data?: T; error?: string } & Record<string, unknown>;

export class PumpStudioApi {
	constructor(private getKey: () => string) {}

	private headers(json: boolean, keyOverride?: string): Record<string, string> {
		const key = keyOverride ?? this.getKey();
		const h: Record<string, string> = {};
		if (json) h["Content-Type"] = "application/json";
		if (key) {
			// The official clients send Bearer; the original desktop app documented X-API-Key.
			h.Authorization = `Bearer ${key}`;
			h["X-API-Key"] = key;
		}
		return h;
	}

	private async request<T>(method: "GET" | "POST", path: string, body?: unknown, keyOverride?: string): Promise<T> {
		let res: Response;
		try {
			res = await fetch(`${BASE}${path}`, {
				method,
				headers: this.headers(body !== undefined, keyOverride),
				body: body !== undefined ? JSON.stringify(body) : undefined,
				signal: AbortSignal.timeout(TIMEOUT_MS),
			});
		} catch (err) {
			const reason = err instanceof Error && err.name === "TimeoutError" ? "timed out" : "is unreachable";
			throw new ApiError(`pump.studio ${reason}. Check your connection and try again.`, 0);
		}
		const text = await res.text();
		let data: unknown;
		try {
			data = text ? JSON.parse(text) : {};
		} catch {
			throw new ApiError(`Unexpected response from pump.studio (HTTP ${res.status})`, res.status);
		}
		const env = data as Envelope<unknown>;
		if (!res.ok) {
			throw new ApiError(friendlyError(res.status, env?.error), res.status);
		}
		if (env && env.ok === false && env.error) {
			throw new ApiError(friendlyError(res.status, env.error), res.status);
		}
		return data as T;
	}

	async getMarket(tab: MarketTab = "all", limit = 50): Promise<MarketToken[]> {
		const q = new URLSearchParams({ tab, limit: String(limit), format: "json" });
		const res = await this.request<Envelope<MarketToken[]> & { tokens?: MarketToken[]; items?: MarketToken[] }>(
			"GET",
			`/api/v1/market?${q}`,
		);
		const list = res.data ?? res.tokens ?? res.items ?? [];
		return Array.isArray(list) ? list.filter((t) => t && typeof t.mint === "string") : [];
	}

	async getDataPoint(mint: string): Promise<DataPoint> {
		const res = await this.request<Envelope<DataPoint>>("GET", `/api/v1/datapoint?mint=${encodeURIComponent(mint.trim())}`);
		const dp = res.data ?? ((res as unknown as DataPoint).mint ? (res as unknown as DataPoint) : undefined);
		if (!dp) throw new ApiError("Token not found on pump.studio", 404);
		return dp;
	}

	async getAnalysisStats(limit = 25): Promise<AnalysisStats> {
		const res = await this.request<Envelope<Partial<AnalysisStats>> & Partial<AnalysisStats>>(
			"GET",
			`/api/v1/analysis?limit=${encodeURIComponent(String(limit))}`,
		);
		const s = (res.data && typeof res.data === "object" ? res.data : res) as Partial<AnalysisStats>;
		const n = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : 0);
		return {
			totalAnalyses: n(s.totalAnalyses),
			uniqueTokens: n(s.uniqueTokens),
			goal: n(s.goal),
			progress: n(s.progress),
			leaderboard: Array.isArray(s.leaderboard) ? (s.leaderboard as LeaderboardEntry[]) : [],
		};
	}

	async submitAnalysis(payload: AnalysisPayload): Promise<SubmitResult> {
		const res = await this.request<SubmitResult & { data?: SubmitResult }>("POST", "/api/v1/analysis/submit", payload);
		// Some endpoints nest the result in data, the official clients read it flat.
		return { ...(res.data ?? {}), ...res, ok: res.ok ?? res.data?.ok ?? true };
	}

	async getContext(mint: string): Promise<{ systemPrompt?: string; context?: string; analysisSchema?: string }> {
		const res = await this.request<Envelope<{ systemPrompt?: string; context?: string; analysisSchema?: string }>>(
			"GET",
			`/api/v1/chat/context?mint=${encodeURIComponent(mint)}`,
		);
		return res.data ?? {};
	}

	async registerKey(name: string, description?: string): Promise<RegisteredKey> {
		const res = await this.request<Envelope<RegisteredKey>>("POST", "/api/v1/keys/register", { name, description }, "");
		if (!res.data?.key) throw new ApiError(res.error ?? "Key registration failed", 0);
		return res.data;
	}

	/** Verify a key by reading the agent profile. */
	async checkKey(key: string): Promise<KeyCheck> {
		if (!key) return { status: "missing" };
		try {
			const res = await this.request<{ ok?: boolean; profile?: { name?: string } | null }>(
				"GET",
				"/api/v1/agent/profile",
				undefined,
				key,
			);
			return { status: "valid", agentName: res.profile?.name ?? undefined };
		} catch (err) {
			if (err instanceof ApiError && (err.status === 401 || err.status === 403)) {
				return { status: "invalid", message: "pump.studio rejected this key" };
			}
			if (err instanceof ApiError && err.status === 0) return { status: "unreachable", message: err.message };
			return { status: "invalid", message: err instanceof Error ? err.message : String(err) };
		}
	}
}

function friendlyError(status: number, serverMessage?: string): string {
	const msg = serverMessage?.trim();
	if (status === 401 || status === 403) return msg ? `Not authorized: ${msg}` : "Not authorized. Check your pump.studio API key in Settings.";
	if (status === 404) return msg || "Not found on pump.studio";
	if (status === 429) return msg ? `Rate limited: ${msg}` : "Rate limited by pump.studio. Wait a moment and try again.";
	if (status >= 500) return msg ? `pump.studio error: ${msg}` : `pump.studio is having trouble (HTTP ${status})`;
	return msg || `Request failed (HTTP ${status})`;
}
