import { afterEach, describe, expect, test } from "bun:test";
import { ApiError, PumpStudioApi } from "../src/bun/api";

const realFetch = globalThis.fetch;
afterEach(() => {
	globalThis.fetch = realFetch;
});

function mockFetch(status: number, body: unknown, seen?: { url?: string; headers?: Record<string, string> }) {
	globalThis.fetch = (async (url: string | URL, init?: RequestInit) => {
		if (seen) {
			seen.url = String(url);
			seen.headers = init?.headers as Record<string, string>;
		}
		return new Response(JSON.stringify(body), { status });
	}) as typeof fetch;
}

describe("PumpStudioApi", () => {
	test("reads the market list from the { ok, data } envelope", async () => {
		const seen: { url?: string } = {};
		mockFetch(200, { ok: true, data: [{ mint: "A".repeat(44), name: "A", symbol: "A" }] }, seen);
		const tokens = await new PumpStudioApi(() => "ps_test").getMarket("live", 10);
		expect(tokens).toHaveLength(1);
		expect(seen.url).toContain("tab=live");
		expect(seen.url).toContain("format=json");
	});

	test("unwraps the DataPoint and encodes the mint", async () => {
		const seen: { url?: string; headers?: Record<string, string> } = {};
		mockFetch(200, { ok: true, data: { mint: "M", name: "N", symbol: "S" } }, seen);
		const dp = await new PumpStudioApi(() => "ps_key").getDataPoint(" M&x ");
		expect(dp.mint).toBe("M");
		expect(seen.url).toContain("mint=M%26x");
		expect(seen.headers?.Authorization).toBe("Bearer ps_key");
	});

	test("turns HTTP errors into readable ApiErrors", async () => {
		mockFetch(429, { error: "slow down" });
		const err = await new PumpStudioApi(() => "").getMarket().catch((e) => e);
		expect(err).toBeInstanceOf(ApiError);
		expect(err.status).toBe(429);
		expect(err.message).toContain("slow down");
	});

	test("treats ok:false with HTTP 200 as an error", async () => {
		mockFetch(200, { ok: false, error: "Token on cooldown" });
		const err = await new PumpStudioApi(() => "ps").submitAnalysis({} as never).catch((e) => e);
		expect(err).toBeInstanceOf(ApiError);
		expect(err.message).toContain("cooldown");
	});
});
