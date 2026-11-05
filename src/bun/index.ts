import Electrobun, { ApplicationMenu, BrowserView, BrowserWindow, Updater, Utils, type ElectrobunEvent } from "electrobun/bun";
import { extractMint } from "../shared/analysis";
import type { AppCommand, PumpStudioRPC, ViewId } from "../shared/rpc-schema";
import type { TrainingState, XpState } from "../shared/types";
import { PumpStudioApi } from "./api";
import { ConfigStore } from "./config";
import { Orchestrator } from "./orchestrator";
import { Trainer } from "./training";

/* ------------------------------------------------------------------ */
/*  Services                                                            */
/* ------------------------------------------------------------------ */

const config = new ConfigStore();
const api = new PumpStudioApi(() => config.get().apiKey);
const orchestrator = new Orchestrator(() => config.get());
const trainer = new Trainer(
	api,
	orchestrator,
	() => config.get(),
	(state) => pushTrainingState(state),
	(xp) => pushXp(xp),
);

/* Declared as functions with explicit return types to break the rpc <-> handler type cycle. */
function pushTrainingState(state: TrainingState): void {
	rpc.send.trainingState(state);
}
function pushXp(xp: XpState): void {
	rpc.send.xp(xp);
}

const WEB_BASE = "https://pump.studio";
const SAFE_URL = /^(https:\/\/|http:\/\/|mailto:)/i;

/* ------------------------------------------------------------------ */
/*  Typed RPC (webview <-> bun)                                         */
/*  Request handlers must throw Error instances: only .message crosses. */
/* ------------------------------------------------------------------ */

const rpc = BrowserView.defineRPC<PumpStudioRPC>({
	maxRequestTime: 10_000,
	handlers: {
		requests: {
			getMarket: ({ tab, limit }) => api.getMarket(tab, Math.min(Math.max(limit ?? 50, 1), 100)),
			getDataPoint: ({ mint }) => {
				const m = extractMint(mint);
				if (!m) throw new Error("That does not look like a Solana mint address");
				return api.getDataPoint(m);
			},
			getAnalysisStats: ({ limit }) => api.getAnalysisStats(limit ?? 25),
			submitAnalysis: async (payload) => {
				const result = await api.submitAnalysis(payload);
				trainer.recordSubmit(payload.mint, result);
				return result;
			},
			orchestrateAnalysis: ({ dataPoint }) => orchestrator.analyze(dataPoint),
			loadConfig: () => config.toPublic(),
			saveConfig: (patch) => {
				config.apply(patch ?? {});
				return config.toPublic();
			},
			checkApiKey: ({ apiKey }) => api.checkKey(apiKey?.trim() || config.get().apiKey),
			registerApiKey: ({ name, description }) => {
				const clean = String(name ?? "").trim().slice(0, 40);
				if (!clean) throw new Error("Give your agent a name");
				return api.registerKey(clean, description?.trim().slice(0, 200));
			},
			getTrainingState: () => trainer.getState(),
			startAutoTraining: ({ targets }) => trainer.start(targets ?? []),
			stopAutoTraining: () => trainer.stop(),
			getXp: () => trainer.getXp(),
			openExternal: ({ url }) => {
				if (!SAFE_URL.test(url)) throw new Error("Blocked a link with an unsupported scheme");
				return Utils.openExternal(url);
			},
			openWebApp: ({ path }) => {
				openWebAppWindow(path);
				return true;
			},
		},
		messages: {
			viewReady: () => {
				// The view (re)loaded: push current state so nothing is stale.
				pushTrainingState(trainer.getState());
				pushXp(trainer.getXp());
			},
		},
	},
});

/* ------------------------------------------------------------------ */
/*  Main window: the local Pump.studio UI                               */
/* ------------------------------------------------------------------ */

const DEV_SERVER_URL = "http://localhost:5173";

async function mainViewUrl(): Promise<string> {
	try {
		if ((await Updater.localInfo.channel()) === "dev") {
			await fetch(DEV_SERVER_URL, { method: "HEAD", signal: AbortSignal.timeout(500) });
			return DEV_SERVER_URL; // `bun run dev:hmr`
		}
	} catch {}
	return "views://mainview/index.html";
}

const mainWin = new BrowserWindow({
	title: "Pump.studio",
	url: await mainViewUrl(),
	frame: { width: 1360, height: 880, x: 120, y: 80 },
	titleBarStyle: "hiddenInset",
	rpc,
});

/* Links with target=_blank or window.open from the local UI go to the system browser. */
Electrobun.events.on(
	`new-window-open-${mainWin.webview.id}`,
	(e: ElectrobunEvent<{ detail: string | { url: string } }, unknown>) => {
		const url = typeof e.data.detail === "string" ? e.data.detail : e.data.detail?.url;
		if (url && SAFE_URL.test(url)) Utils.openExternal(url);
	},
);

function sendCommand(cmd: AppCommand): void {
	mainWin.focus();
	rpc.send.command(cmd);
}

/* ------------------------------------------------------------------ */
/*  Web window: pump.studio itself (the original desktop wrapper)       */
/* ------------------------------------------------------------------ */

const WEB_CSS = `
	html { background: #0b0c0f; }
	::-webkit-scrollbar { width: 8px; height: 8px; }
	::-webkit-scrollbar-thumb { background: #353a43; border-radius: 4px; border: 2px solid transparent; background-clip: padding-box; }
	::-webkit-scrollbar-track { background: transparent; }
`;
const WEB_INJECT = `(() => {
	if (document.getElementById('__ps_desktop')) return;
	const s = document.createElement('style');
	s.id = '__ps_desktop';
	s.textContent = ${JSON.stringify(WEB_CSS)};
	(document.head || document.documentElement).appendChild(s);
})();`;

let webWin: BrowserWindow | null = null;

function openWebAppWindow(path = "/market") {
	const clean = path.startsWith("/") ? path : `/${path}`;
	const url = `${WEB_BASE}${clean}`;
	if (webWin) {
		webWin.webview.loadURL(url);
		webWin.focus();
		return;
	}
	const win = new BrowserWindow({
		title: "pump.studio",
		url,
		frame: { width: 1320, height: 880, x: 180, y: 120 },
		titleBarStyle: "default",
		sandbox: true, // remote content: no RPC bridge into the app
	});
	const inject = () => {
		try {
			win.webview.executeJavascript(WEB_INJECT);
		} catch {}
	};
	win.webview.on("dom-ready", inject);
	win.webview.on("did-navigate", inject);
	win.webview.on("did-navigate-in-page", inject);
	Electrobun.events.on(
		`new-window-open-${win.webview.id}`,
		(e: ElectrobunEvent<{ detail: string | { url: string } }, unknown>) => {
			const u = typeof e.data.detail === "string" ? e.data.detail : e.data.detail?.url;
			if (!u) return;
			if (u.startsWith(WEB_BASE)) win.webview.loadURL(u);
			else if (SAFE_URL.test(u)) Utils.openExternal(u);
		},
	);
	win.on("close", () => {
		webWin = null;
	});
	webWin = win;
}

/* ------------------------------------------------------------------ */
/*  Native menus                                                        */
/* ------------------------------------------------------------------ */

const isMac = process.platform === "darwin";
const mod = isMac ? "Command" : "Control";

const NAV: Record<string, ViewId> = {
	"nav:tokens": "tokens",
	"nav:analyze": "analyze",
	"nav:auto": "auto",
	"nav:leaderboard": "leaderboard",
	"nav:settings": "settings",
};

ApplicationMenu.setApplicationMenu([
	{
		label: "Pump.studio",
		submenu: [
			{ role: "about" },
			{ type: "separator" },
			{ label: "Settings…", action: "nav:settings", accelerator: `${mod}+,` },
			{ type: "separator" },
			...(isMac
				? [
						{ role: "hide", accelerator: "Command+H" },
						{ role: "hideOthers", accelerator: "Command+Option+H" },
						{ role: "showAll" },
						{ type: "separator" as const },
					]
				: []),
			{ role: "quit", accelerator: `${mod}+Q` },
		],
	},
	{
		label: "Edit",
		submenu: [
			{ role: "undo" },
			{ role: "redo" },
			{ type: "separator" },
			{ role: "cut" },
			{ role: "copy" },
			{ role: "paste" },
			{ role: "selectAll" },
		],
	},
	{
		label: "View",
		submenu: [
			{ label: "Tokens", action: "nav:tokens", accelerator: `${mod}+1` },
			{ label: "Analyze", action: "nav:analyze", accelerator: `${mod}+2` },
			{ label: "Auto Train", action: "nav:auto", accelerator: `${mod}+3` },
			{ label: "Leaderboard", action: "nav:leaderboard", accelerator: `${mod}+4` },
			{ type: "separator" },
			{ label: "Command Palette", action: "cmd:palette", accelerator: `${mod}+K` },
			{ label: "Toggle Sidebar", action: "cmd:sidebar", accelerator: `${mod}+B` },
			{ label: "Refresh", action: "cmd:refresh", accelerator: `${mod}+R` },
			{ type: "separator" },
			{ label: "Reload Window", action: "view:reload", accelerator: `${mod}+Shift+R` },
			{ label: "Developer Tools", action: "view:devtools", accelerator: `${mod}+Option+I` },
			{ type: "separator" },
			{ role: "toggleFullScreen", accelerator: isMac ? "Command+Control+F" : "F11" },
		],
	},
	{
		label: "Go",
		submenu: [
			{ label: "Open pump.studio Website", action: "web:/market", accelerator: `${mod}+Shift+O` },
			{ type: "separator" },
			{ label: "Market", action: "web:/market" },
			{ label: "Livestreams", action: "web:/streams" },
			{ label: "Agents Leaderboard", action: "web:/agents?tab=leaderboard" },
			{ label: "DataPoint", action: "web:/datapoint" },
		],
	},
	{
		label: "Window",
		submenu: [
			{ role: "minimize", accelerator: `${mod}+M` },
			{ role: "zoom" },
			{ role: "close", accelerator: `${mod}+W` },
			...(isMac ? [{ role: "bringAllToFront" }] : []),
		],
	},
	{
		label: "Help",
		submenu: [
			{ label: "Pump.studio Docs", action: "link:docs" },
			{ label: "Get an API Key", action: "link:keys" },
			{ label: "Report an Issue", action: "link:issues" },
			{ type: "separator" },
			{ label: "@pumpdotstudio on X", action: "link:x" },
		],
	},
]);

/* Menu item `data` is single-use in Electrobun 1.13 (cleared after the first click),
   so targets are encoded in the action string instead. */
const LINKS: Record<string, string> = {
	"link:docs": "https://pump.studio/docs",
	"link:keys": "https://pump.studio/agents",
	"link:issues": "https://github.com/tabbink2025/PumpStudio/issues",
	"link:x": "https://x.com/pumpdotstudio",
};

type MenuClick = ElectrobunEvent<{ id?: number; action: string }, { allow: boolean }>;

Electrobun.events.on("application-menu-clicked", (e: MenuClick) => {
	const { action } = e.data;
	const view = NAV[action];
	if (view) return sendCommand({ type: "navigate", view });
	if (action.startsWith("web:")) return openWebAppWindow(action.slice(4));
	if (LINKS[action]) {
		Utils.openExternal(LINKS[action]!);
		return;
	}
	switch (action) {
		case "cmd:palette":
			return sendCommand({ type: "openPalette" });
		case "cmd:sidebar":
			return sendCommand({ type: "toggleSidebar" });
		case "cmd:refresh":
			return sendCommand({ type: "refresh" });
		case "view:reload":
			return mainWin.webview.executeJavascript("location.reload()");
		case "view:devtools":
			return mainWin.webview.toggleDevTools();
	}
});

/* ------------------------------------------------------------------ */
/*  Lifecycle: closing the main window quits, even with the web window. */
/* ------------------------------------------------------------------ */

mainWin.on("close", () => {
	trainer.stop();
	Utils.quit();
});

console.log("[app] Pump.studio started");
