import { Component, useEffect, useRef, useState, type ReactNode } from "react";
import { AlertTriangle } from "lucide-react";
import type { ViewId } from "../shared/rpc-schema";
import { CommandPalette } from "./components/shell/CommandPalette";
import { ShortcutsDialog } from "./components/shell/ShortcutsDialog";
import { Sidebar } from "./components/shell/Sidebar";
import { Button } from "./components/ui/Button";
import { Toaster } from "./components/ui/overlays";
import { isModKey, isTypingTarget, overlayOpen } from "./lib/hotkeys";
import { bun, isDemo, on } from "./lib/rpc";
import { useApp } from "./lib/store";
import { AnalyzeView } from "./views/AnalyzeView";
import { AutoTrainView } from "./views/AutoTrainView";
import { LeaderboardView } from "./views/LeaderboardView";
import { SettingsView } from "./views/SettingsView";
import { TokensView } from "./views/TokensView";

const VIEWS: Array<{ id: ViewId; render: (active: boolean) => ReactNode }> = [
	{ id: "tokens", render: (a) => <TokensView active={a} /> },
	{ id: "analyze", render: (a) => <AnalyzeView active={a} /> },
	{ id: "auto", render: (a) => <AutoTrainView active={a} /> },
	{ id: "leaderboard", render: (a) => <LeaderboardView active={a} /> },
	{ id: "settings", render: (a) => <SettingsView active={a} /> },
];

const NUM_TO_VIEW: Record<string, ViewId> = { "1": "tokens", "2": "analyze", "3": "auto", "4": "leaderboard", "5": "settings" };
const G_TO_VIEW: Record<string, ViewId> = { t: "tokens", a: "analyze", r: "auto", l: "leaderboard", s: "settings" };

/* ---- Theme + density ------------------------------------------------- */

function useAppearance() {
	const theme = useApp((s) => s.config?.prefs.theme ?? "system");
	const density = useApp((s) => s.config?.prefs.density ?? "comfortable");
	useEffect(() => {
		const root = document.documentElement;
		const mq = window.matchMedia("(prefers-color-scheme: light)");
		const apply = () => {
			root.dataset.theme = theme === "system" ? (mq.matches ? "light" : "dark") : theme;
		};
		apply();
		if (theme !== "system") return;
		mq.addEventListener("change", apply);
		return () => mq.removeEventListener("change", apply);
	}, [theme]);
	useEffect(() => {
		document.documentElement.dataset.density = density;
	}, [density]);
}

/* ---- Bridge to the Bun process --------------------------------------- */

function useBridge() {
	useEffect(() => {
		const s = useApp.getState();
		void s.loadConfig();
		bun.getTrainingState().then(s.setTraining, () => {});
		bun.getXp().then(s.setXp, () => {});
		const offs = [
			on("trainingState", (t) => useApp.getState().setTraining(t)),
			on("xp", (x) => useApp.getState().setXp(x)),
			on("command", (cmd) => {
				const st = useApp.getState();
				if (cmd.type === "navigate") st.setView(cmd.view);
				if (cmd.type === "openPalette") st.setPalette(true);
				if (cmd.type === "refresh") st.refresh();
				if (cmd.type === "toggleSidebar") st.setPrefs({ sidebarCollapsed: !st.config?.prefs.sidebarCollapsed });
			}),
		];
		bun.viewReady();
		return () => offs.forEach((off) => off());
	}, []);
}

/* ---- Global shortcuts -------------------------------------------------- */

function useGlobalKeys() {
	const gPending = useRef<number>(0);
	useEffect(() => {
		const onKey = (e: KeyboardEvent) => {
			const st = useApp.getState();
			const k = e.key.toLowerCase();
			if (isModKey(e) && k === "k") {
				e.preventDefault();
				st.setPalette(!st.paletteOpen);
				return;
			}
			if (isModKey(e) && !e.shiftKey && NUM_TO_VIEW[e.key]) {
				e.preventDefault();
				st.setView(NUM_TO_VIEW[e.key]!);
				return;
			}
			if (isModKey(e) && k === ",") {
				e.preventDefault();
				st.setView("settings");
				return;
			}
			if (isModKey(e) && k === "b") {
				e.preventDefault();
				st.setPrefs({ sidebarCollapsed: !st.config?.prefs.sidebarCollapsed });
				return;
			}
			if (isModKey(e) && !e.shiftKey && k === "r") {
				e.preventDefault();
				st.refresh();
				return;
			}
			if (isTypingTarget(e) || overlayOpen() || e.metaKey || e.ctrlKey || e.altKey) return;
			if (e.key === "/") {
				e.preventDefault();
				st.setPalette(true);
			} else if (e.key === "?") {
				e.preventDefault();
				st.setShortcuts(true);
			} else if (k === "g") {
				gPending.current = Date.now();
			} else if (gPending.current && Date.now() - gPending.current < 1200 && G_TO_VIEW[k]) {
				e.preventDefault();
				gPending.current = 0;
				st.setView(G_TO_VIEW[k]!);
			}
		};
		window.addEventListener("keydown", onKey);
		return () => window.removeEventListener("keydown", onKey);
	}, []);
}

/* ---- Error boundary ---------------------------------------------------- */

class ErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
	state = { error: null as Error | null };
	static getDerivedStateFromError(error: Error) {
		return { error };
	}
	componentDidCatch(error: Error) {
		console.error("[ui] crashed:", error);
	}
	render() {
		if (!this.state.error) return this.props.children;
		return (
			<div className="flex h-full flex-col items-center justify-center gap-4 p-8 text-center">
				<AlertTriangle className="size-8 text-warn" />
				<div>
					<h1 className="text-lg font-semibold text-fg">This screen hit an error</h1>
					<p className="mt-1 max-w-md text-base text-fg-3">{this.state.error.message}</p>
				</div>
				<Button variant="primary" onClick={() => this.setState({ error: null })}>
					Try again
				</Button>
			</div>
		);
	}
}

/* ---- App ----------------------------------------------------------------- */

export default function App() {
	const view = useApp((s) => s.view);
	const configLoaded = useApp((s) => !!s.config);
	const configError = useApp((s) => s.configError);
	// Mount a screen the first time it is visited, then keep it alive so
	// drafts, filters and scroll positions survive navigation.
	const [visited, setVisited] = useState<Set<ViewId>>(() => new Set([view]));
	useEffect(() => {
		setVisited((v) => (v.has(view) ? v : new Set(v).add(view)));
	}, [view]);

	useAppearance();
	useBridge();
	useGlobalKeys();

	if (!configLoaded && configError) {
		return (
			<div className="flex h-full flex-col items-center justify-center gap-3 p-8 text-center">
				<AlertTriangle className="size-8 text-warn" />
				<h1 className="text-lg font-semibold text-fg">Could not start Pump.studio</h1>
				<p className="max-w-md text-base text-fg-3">{configError}</p>
				<Button onClick={() => void useApp.getState().loadConfig()}>Retry</Button>
			</div>
		);
	}

	return (
		<div className="flex h-full bg-bg text-fg">
			<Sidebar />
			<main className="relative flex min-w-0 flex-1 flex-col overflow-hidden">
				{isDemo && (
					<div className="shrink-0 border-b border-warn/25 bg-warn/5 px-5 py-1.5 text-sm text-warn">
						Demo mode: generated sample data, nothing is sent to pump.studio. Run the desktop app to use your real account.
					</div>
				)}
				<ErrorBoundary>
					{VIEWS.map(({ id, render }) =>
						visited.has(id) ? (
							<section
								key={id}
								aria-hidden={view !== id}
								className={view === id ? "flex min-h-0 flex-1 flex-col" : "hidden"}
							>
								{render(view === id)}
							</section>
						) : null,
					)}
				</ErrorBoundary>
			</main>
			<CommandPalette />
			<ShortcutsDialog />
			<Toaster />
		</div>
	);
}
