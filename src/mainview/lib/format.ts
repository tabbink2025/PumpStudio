/* ------------------------------------------------------------------ */
/*  Number + time formatting. en-US on purpose: crypto UIs (and         */
/*  pump.studio itself) use $1.2M / 4.5% conventions everywhere.        */
/* ------------------------------------------------------------------ */

const compactUsd = new Intl.NumberFormat("en-US", {
	style: "currency",
	currency: "USD",
	notation: "compact",
	maximumFractionDigits: 1,
});
const compactNum = new Intl.NumberFormat("en-US", { notation: "compact", maximumFractionDigits: 1 });
const intNum = new Intl.NumberFormat("en-US", { maximumFractionDigits: 0 });

const DASH = "—";

function finite(n: unknown): n is number {
	return typeof n === "number" && Number.isFinite(n);
}

/** $1.2M, $45.3K, $812 */
export function usdCompact(n: unknown): string {
	if (!finite(n)) return DASH;
	if (Math.abs(n) < 1000) return `$${n.toFixed(n < 10 ? 2 : 0)}`;
	return compactUsd.format(n);
}

/** Token prices: keeps significant digits for tiny values ($0.0₅4521). */
export function usdPrice(n: unknown): string {
	if (!finite(n)) return DASH;
	if (n === 0) return "$0";
	if (n >= 1) return `$${n.toLocaleString("en-US", { maximumFractionDigits: 2 })}`;
	if (n >= 0.01) return `$${n.toFixed(4)}`;
	const exp = Math.floor(Math.log10(n));
	const zeros = -exp - 1;
	const digits = Math.round(n * 10 ** (-exp + 3))
		.toString()
		.slice(0, 4);
	if (zeros >= 4) return `$0.0${subscript(zeros)}${digits}`;
	return `$${n.toFixed(zeros + 4)}`;
}

function subscript(n: number): string {
	return String(n)
		.split("")
		.map((d) => "₀₁₂₃₄₅₆₇₈₉"[Number(d)])
		.join("");
}

/** 12.4K, 980 */
export function compact(n: unknown): string {
	if (!finite(n)) return DASH;
	return Math.abs(n) < 1000 ? intNum.format(n) : compactNum.format(n);
}

export function integer(n: unknown): string {
	return finite(n) ? intNum.format(n) : DASH;
}

/** +4.2% / −3.1% (true minus sign) */
export function pctSigned(n: unknown, digits = 1): string {
	if (!finite(n)) return DASH;
	const abs = Math.abs(n);
	const s = abs >= 1000 ? compactNum.format(abs) : abs.toFixed(digits);
	if (n > 0) return `+${s}%`;
	if (n < 0) return `−${s}%`;
	return `${s}%`;
}

export function pct(n: unknown, digits = 1): string {
	return finite(n) ? `${n.toFixed(digits)}%` : DASH;
}

/** "4m", "2h", "3d" */
export function ageShort(ts: unknown, now = Date.now()): string {
	if (!finite(ts) || ts <= 0) return DASH;
	const s = Math.max(0, (now - ts) / 1000);
	if (s < 60) return `${Math.floor(s)}s`;
	if (s < 3600) return `${Math.floor(s / 60)}m`;
	if (s < 86400) return `${Math.floor(s / 3600)}h`;
	if (s < 86400 * 30) return `${Math.floor(s / 86400)}d`;
	return `${Math.floor(s / (86400 * 30))}mo`;
}

/** "just now", "12s ago", "3m ago" */
export function ago(ts: unknown, now = Date.now()): string {
	if (!finite(ts)) return "never";
	const s = Math.max(0, Math.round((now - ts) / 1000));
	if (s < 5) return "just now";
	if (s < 60) return `${s}s ago`;
	if (s < 3600) return `${Math.floor(s / 60)}m ago`;
	return `${Math.floor(s / 3600)}h ago`;
}

export function clockTime(ts: number): string {
	return new Date(ts).toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit", second: "2-digit" });
}

export function absoluteTime(ts: unknown): string {
	return finite(ts) ? new Date(ts).toLocaleString() : "";
}

export function duration(ms: number): string {
	const s = Math.max(0, Math.round(ms / 1000));
	if (s < 60) return `${s}s`;
	const m = Math.floor(s / 60);
	if (m < 60) return `${m}m ${s % 60}s`;
	return `${Math.floor(m / 60)}h ${m % 60}m`;
}

export function titleCase(s: string): string {
	return s.replace(/_/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());
}

export function plural(n: number, one: string, many = `${one}s`): string {
	return `${integer(n)} ${n === 1 ? one : many}`;
}
