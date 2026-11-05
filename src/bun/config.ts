import { chmodSync, existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "fs";
import { homedir } from "os";
import { join } from "path";
import type { AgentConfig, ConfigPatch, Orchestrator, PublicConfig, UiPrefs } from "../shared/types";

/* ------------------------------------------------------------------ */
/*  Persistent config: ~/.pump-studio/trainer.json                      */
/*                                                                      */
/*  Holds API keys, so the directory is 0700 and the file 0600. Keys    */
/*  never leave the Bun process: the view only gets a masked copy.      */
/* ------------------------------------------------------------------ */

const CONFIG_DIR = join(homedir(), ".pump-studio");
const CONFIG_FILE = join(CONFIG_DIR, "trainer.json");

export const DEFAULT_CLAUDE_MODEL = "claude-opus-5-5";
export const DEFAULT_OPENAI_MODEL = "gpt-5.5";
export const MIN_INTERVAL_SEC = 6;

const DEFAULT_PREFS: UiPrefs = {
	theme: "system",
	density: "comfortable",
	sidebarCollapsed: false,
	watchlist: [],
	recentMints: [],
};

function defaults(): AgentConfig {
	return {
		apiKey: process.env.PUMP_STUDIO_API_KEY ?? "",
		orchestrator: "manual",
		claudeApiKey: process.env.ANTHROPIC_API_KEY ?? "",
		openaiApiKey: process.env.OPENAI_API_KEY ?? "",
		claudeModel: DEFAULT_CLAUDE_MODEL,
		openaiModel: DEFAULT_OPENAI_MODEL,
		analyzeInterval: 10,
		skipRecentlyAnalyzed: true,
		prefs: { ...DEFAULT_PREFS },
	};
}

export function maskKey(key: string): string {
	if (!key) return "";
	if (key.length <= 8) return "•".repeat(key.length);
	return `${key.slice(0, 3)}…${key.slice(-4)}`;
}

function uniqStrings(v: unknown, max: number): string[] {
	if (!Array.isArray(v)) return [];
	return [...new Set(v.filter((x): x is string => typeof x === "string" && x.length > 0 && x.length < 100))].slice(0, max);
}

function normalizeOrchestrator(v: unknown): Orchestrator {
	if (v === "claude" || v === "manual") return v;
	if (v === "openai" || v === "codex") return "openai"; // "codex" was the old name
	return "manual";
}

export class ConfigStore {
	private config: AgentConfig;

	constructor() {
		this.config = this.load();
	}

	get(): AgentConfig {
		return { ...this.config, prefs: { ...this.config.prefs } };
	}

	toPublic(): PublicConfig {
		const c = this.config;
		return {
			apiKeySet: !!c.apiKey,
			apiKeyHint: maskKey(c.apiKey),
			orchestrator: c.orchestrator,
			claudeKeySet: !!c.claudeApiKey,
			claudeKeyHint: maskKey(c.claudeApiKey),
			openaiKeySet: !!c.openaiApiKey,
			openaiKeyHint: maskKey(c.openaiApiKey),
			claudeModel: c.claudeModel,
			openaiModel: c.openaiModel,
			analyzeInterval: c.analyzeInterval,
			skipRecentlyAnalyzed: c.skipRecentlyAnalyzed,
			prefs: { ...c.prefs },
			configPath: CONFIG_FILE,
		};
	}

	/** Apply a validated patch from the view. */
	apply(patch: ConfigPatch): void {
		const next = this.get();
		const secret = (v: string | null | undefined, current: string) =>
			v === undefined ? current : v === null ? "" : String(v).trim();
		next.apiKey = secret(patch.apiKey, next.apiKey);
		next.claudeApiKey = secret(patch.claudeApiKey, next.claudeApiKey);
		next.openaiApiKey = secret(patch.openaiApiKey, next.openaiApiKey);
		if (patch.orchestrator !== undefined) next.orchestrator = normalizeOrchestrator(patch.orchestrator);
		if (typeof patch.claudeModel === "string" && patch.claudeModel.trim()) next.claudeModel = patch.claudeModel.trim().slice(0, 80);
		if (typeof patch.openaiModel === "string" && patch.openaiModel.trim()) next.openaiModel = patch.openaiModel.trim().slice(0, 80);
		if (typeof patch.analyzeInterval === "number" && Number.isFinite(patch.analyzeInterval)) {
			next.analyzeInterval = Math.round(Math.min(600, Math.max(MIN_INTERVAL_SEC, patch.analyzeInterval)));
		}
		if (typeof patch.skipRecentlyAnalyzed === "boolean") next.skipRecentlyAnalyzed = patch.skipRecentlyAnalyzed;
		if (patch.prefs) {
			const p = patch.prefs;
			if (p.theme === "system" || p.theme === "dark" || p.theme === "light") next.prefs.theme = p.theme;
			if (p.density === "compact" || p.density === "comfortable") next.prefs.density = p.density;
			if (typeof p.sidebarCollapsed === "boolean") next.prefs.sidebarCollapsed = p.sidebarCollapsed;
			if (p.watchlist) next.prefs.watchlist = uniqStrings(p.watchlist, 500);
			if (p.recentMints) next.prefs.recentMints = uniqStrings(p.recentMints, 20);
		}
		this.config = next;
		this.persist();
	}

	private persist(): void {
		try {
			if (!existsSync(CONFIG_DIR)) mkdirSync(CONFIG_DIR, { recursive: true, mode: 0o700 });
			const tmp = `${CONFIG_FILE}.tmp`;
			writeFileSync(tmp, JSON.stringify(this.config, null, 2), { mode: 0o600 });
			renameSync(tmp, CONFIG_FILE);
			chmodSync(CONFIG_FILE, 0o600);
		} catch (err) {
			console.error("[config] save failed:", err);
			throw new Error("Could not save settings to disk");
		}
	}

	private load(): AgentConfig {
		const base = defaults();
		try {
			if (!existsSync(CONFIG_FILE)) return base;
			try {
				chmodSync(CONFIG_FILE, 0o600); // tighten files written by older versions
			} catch {}
			const raw = JSON.parse(readFileSync(CONFIG_FILE, "utf-8")) as Record<string, unknown>;
			const str = (v: unknown, fb: string) => (typeof v === "string" ? v : fb);
			const prefs = (raw.prefs ?? {}) as Partial<UiPrefs>;
			return {
				apiKey: str(raw.apiKey, base.apiKey),
				orchestrator: normalizeOrchestrator(raw.orchestrator),
				claudeApiKey: str(raw.claudeApiKey, base.claudeApiKey),
				openaiApiKey: str(raw.openaiApiKey ?? raw.codexApiKey, base.openaiApiKey),
				claudeModel: str(raw.claudeModel, base.claudeModel) || base.claudeModel,
				openaiModel: str(raw.openaiModel, base.openaiModel) || base.openaiModel,
				analyzeInterval:
					typeof raw.analyzeInterval === "number" ? Math.max(MIN_INTERVAL_SEC, raw.analyzeInterval) : base.analyzeInterval,
				skipRecentlyAnalyzed:
					typeof raw.skipRecentlyAnalyzed === "boolean" ? raw.skipRecentlyAnalyzed : base.skipRecentlyAnalyzed,
				prefs: {
					theme: prefs.theme === "dark" || prefs.theme === "light" ? prefs.theme : "system",
					density: prefs.density === "compact" ? "compact" : "comfortable",
					sidebarCollapsed: prefs.sidebarCollapsed === true,
					watchlist: uniqStrings(prefs.watchlist, 500),
					recentMints: uniqStrings(prefs.recentMints, 20),
				},
			};
		} catch (err) {
			console.error("[config] load failed, using defaults:", err);
			return base;
		}
	}
}
