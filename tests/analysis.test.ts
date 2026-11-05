import { describe, expect, test } from "bun:test";
import {
	MAX_RISK_FACTORS,
	RISK_FACTOR_IDS,
	SUMMARY_MIN,
	buildPayload,
	computeDefaults,
	extractMint,
	sanitizeFields,
	validateFields,
} from "../src/shared/analysis";
import type { DataPoint } from "../src/shared/types";
import { extractJson } from "../src/bun/orchestrator";

const MINT = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";

function dp(over: Partial<DataPoint> = {}): DataPoint {
	return {
		mint: MINT,
		name: "Test",
		symbol: "TEST",
		priceUsd: 0.00002,
		priceChange1h: 4,
		priceChange24h: 12,
		marketCap: 40_000,
		liquidity: 12_000,
		volume24h: 25_000,
		buys24h: 300,
		sells24h: 120,
		holderCount: 420,
		bondingComplete: false,
		bondingProgress: 58,
		top10Holding: 34,
		twitter: "https://x.com/test",
		...over,
	};
}

describe("extractMint", () => {
	test("accepts a bare mint", () => expect(extractMint(`  ${MINT}\n`)).toBe(MINT));
	test("pulls the mint out of explorer URLs", () => {
		expect(extractMint(`https://pump.fun/coin/${MINT}`)).toBe(MINT);
		expect(extractMint(`https://dexscreener.com/solana/${MINT}?x=1`)).toBe(MINT);
		expect(extractMint(`https://pump.studio/datapoint?mint=${MINT}`)).toBe(MINT);
	});
	test("rejects garbage", () => {
		expect(extractMint("hello world")).toBeNull();
		expect(extractMint("0OIl0OIl0OIl0OIl0OIl0OIl0OIl0OIl")).toBeNull(); // not base58
	});
});

describe("computeDefaults", () => {
	test("produces valid, submittable fields", () => {
		const f = computeDefaults(dp());
		expect(validateFields(f)).toEqual({});
		expect(f.riskFactors.length).toBeGreaterThan(0);
		expect(f.riskFactors.length).toBeLessThanOrEqual(MAX_RISK_FACTORS);
		for (const r of f.riskFactors) expect(RISK_FACTOR_IDS).toContain(r);
		expect(f.summary.length).toBeGreaterThanOrEqual(SUMMARY_MIN);
	});

	test("survives an all-null snapshot (API returns nulls)", () => {
		const f = computeDefaults(
			dp({
				priceUsd: null,
				priceChange1h: null,
				priceChange24h: null,
				marketCap: null,
				liquidity: null,
				volume24h: null,
				buys24h: null,
				sells24h: null,
				holderCount: null,
				bondingProgress: null,
				top10Holding: null,
			}),
		);
		expect(Number.isFinite(f.score)).toBe(true);
		expect(f.buyPressure).toBe(50);
		expect(f.liquidityDepth).toBe("dry");
	});

	test("flags whale dominance", () => {
		const f = computeDefaults(dp({ top10Holding: 91 }));
		expect(f.riskFactors).toContain("whale_dominance");
		expect(f.holderConcentration).toBe("whale_dominated");
	});
});

describe("sanitizeFields", () => {
	test("drops invalid AI values and keeps valid ones", () => {
		const fallback = computeDefaults(dp());
		const out = sanitizeFields(
			{
				sentiment: "moon",
				score: 140,
				riskLevel: "low",
				riskFactors: ["rug_pull_risk", "low_liquidity", "low_liquidity", "pump_and_dump"],
				buyPressure: 71.6,
				summary: "too short",
			},
			fallback,
		);
		expect(out.sentiment).toBe(fallback.sentiment);
		expect(out.score).toBe(100);
		expect(out.riskLevel).toBe("low");
		expect(out.riskFactors).toEqual(["low_liquidity"]);
		expect(out.buyPressure).toBe(72);
		expect(out.summary).toBe(fallback.summary);
	});
});

describe("buildPayload", () => {
	test("matches the submit schema and coerces nulls to 0", () => {
		const d = dp({ liquidity: null, top10Holding: 33.333 });
		const p = buildPayload(d, computeDefaults(d), 1_700_000_000_000);
		expect(p.snapshot.liquidity).toBe(0);
		expect(p.snapshot.top10HolderPct).toBe(33.3);
		expect(p.snapshot.snapshotAt).toBe(1_700_000_000_000);
		expect(Object.keys(p.quant).sort()).toEqual(
			["buyPressure", "holderConcentration", "liquidityDepth", "riskFactors", "riskLevel", "trendDirection", "volatilityScore", "volumeProfile"],
		);
	});
});

describe("extractJson", () => {
	test("tolerates code fences and prose", () => {
		expect(extractJson('Here you go:\n```json\n{"score": 61}\n```')).toEqual({ score: 61 });
	});
	test("throws without JSON", () => {
		expect(() => extractJson("no json here")).toThrow();
	});
});
