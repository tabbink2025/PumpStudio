import { useEffect, useId, useState } from "react";
import { Cpu, Database, Info, Keyboard, Monitor, Moon, Palette, Sun } from "lucide-react";
import type { Density, PublicConfig, ThemePref, UiPrefs } from "../../../shared/types";
import { integer, plural } from "../../lib/format";
import { isDemo } from "../../lib/rpc";
import { useApp } from "../../lib/store";
import { Button } from "../ui/Button";
import { Badge, FieldMessage, Kbd, MOD, Segmented, Skeleton } from "../ui/primitives";
import {
	ConfirmDialog,
	CopyButton,
	LinkButton,
	openLink,
	SavedMark,
	SettingRow,
	SettingsSection,
	SwitchRow,
	useSavedFlash,
} from "./shared";

export const MIN_INTERVAL = 6;
export const MAX_INTERVAL = 600;
const ANALYZED_STORAGE_KEY = "pumpstudio.analyzed.v1";
export const APP_VERSION = "0.2.0";

/* ---- Auto train --------------------------------------------------------- */

export function AutoTrainSection({ id, config }: { id: string; config: PublicConfig }) {
	const saveConfig = useApp((s) => s.saveConfig);
	const [saved, markSaved] = useSavedFlash();
	const inputId = useId();
	const [text, setText] = useState(String(config.analyzeInterval));
	useEffect(() => setText(String(config.analyzeInterval)), [config.analyzeInterval]);

	const n = Number(text);
	const valid = text.trim() !== "" && Number.isFinite(n) && n >= MIN_INTERVAL && n <= MAX_INTERVAL;
	const perMin = valid ? 60 / n : 60 / config.analyzeInterval;
	const perMinText = perMin >= 1 ? integer(Math.floor(perMin * 10) / 10) : `less than 1`;

	async function commit() {
		if (!valid) {
			// Clamp instead of rejecting, so the field never stays broken.
			const clamped = Number.isFinite(n) && text.trim() !== "" ? Math.min(MAX_INTERVAL, Math.max(MIN_INTERVAL, Math.round(n))) : config.analyzeInterval;
			setText(String(clamped));
			if (clamped === config.analyzeInterval) return;
			const next = await saveConfig({ analyzeInterval: clamped }, { silent: true });
			if (next) markSaved("interval");
			return;
		}
		const v = Math.round(n);
		setText(String(v));
		if (v === config.analyzeInterval) return;
		const next = await saveConfig({ analyzeInterval: v }, { silent: true });
		if (next) markSaved("interval");
		else setText(String(config.analyzeInterval));
	}

	async function toggleSkip(v: boolean) {
		const next = await saveConfig({ skipRecentlyAnalyzed: v }, { silent: true });
		if (next) markSaved("skip");
	}

	return (
		<SettingsSection
			id={id}
			title="Auto train"
			icon={<Cpu className="size-4" aria-hidden />}
			description="Defaults for unattended runs. You start and stop runs on the Auto Train screen."
		>
			<SettingRow
				label="Time between tokens"
				htmlFor={inputId}
				saved={saved === "interval"}
				description={
					<>
						pump.studio accepts up to 10 submissions per minute, so the minimum is {MIN_INTERVAL} s.
						{valid && (
							<>
								{" "}
								At <span className="num">{integer(n)}</span> s that is about{" "}
								<span className="num font-medium text-fg-2">{perMinText}</span> {perMin === 1 ? "token" : "tokens"} per
								minute.
							</>
						)}
					</>
				}
				control={
					<div className="flex items-center gap-2">
						<div className="field flex h-8 w-[96px] items-center pr-2.5" aria-invalid={!valid || undefined}>
							<input
								id={inputId}
								type="number"
								inputMode="numeric"
								min={MIN_INTERVAL}
								max={MAX_INTERVAL}
								step={1}
								value={text}
								aria-invalid={!valid || undefined}
								aria-describedby={!valid ? `${inputId}-err` : undefined}
								onChange={(e) => setText(e.target.value)}
								onBlur={() => void commit()}
								onKeyDown={(e) => {
									if (e.key === "Enter") {
										e.preventDefault();
										void commit();
									}
								}}
								className="num h-full min-w-0 flex-1 bg-transparent px-2.5 text-right text-base text-fg outline-none"
							/>
							<span className="text-sm text-fg-3" aria-hidden>
								s
							</span>
						</div>
					</div>
				}
			>
				{!valid && (
					<FieldMessage error id={`${inputId}-err`}>
						Use {MIN_INTERVAL} to {MAX_INTERVAL} seconds. We round it when you leave the field.
					</FieldMessage>
				)}
			</SettingRow>
			<SwitchRow
				checked={config.skipRecentlyAnalyzed}
				onChange={(v) => void toggleSkip(v)}
				saved={saved === "skip"}
				label="Skip tokens analyzed in the last 2 minutes"
				description="Avoids duplicate submissions that pump.studio would not reward."
			/>
		</SettingsSection>
	);
}

/* ---- Appearance ------------------------------------------------------------ */

const THEMES = [
	{ value: "system", label: <><Monitor className="size-3.5" aria-hidden />System</> },
	{ value: "light", label: <><Sun className="size-3.5" aria-hidden />Light</> },
	{ value: "dark", label: <><Moon className="size-3.5" aria-hidden />Dark</> },
] as const satisfies ReadonlyArray<{ value: ThemePref; label: React.ReactNode }>;

const DENSITIES = [
	{ value: "comfortable", label: "Comfortable" },
	{ value: "compact", label: "Compact" },
] as const satisfies ReadonlyArray<{ value: Density; label: string }>;

/** Optimistic prefs update with inline feedback (store.setPrefs has no result). */
export function usePrefSaver(markSaved: (k: string) => void) {
	const saveConfig = useApp((s) => s.saveConfig);
	return async (key: string, prefs: Partial<UiPrefs>) => {
		const cur = useApp.getState().config;
		if (cur) useApp.setState({ config: { ...cur, prefs: { ...cur.prefs, ...prefs } } });
		const next = await saveConfig({ prefs }, { silent: true });
		if (next) markSaved(key);
		return !!next;
	};
}

export function AppearanceSection({ id, config }: { id: string; config: PublicConfig }) {
	const [saved, markSaved] = useSavedFlash();
	const save = usePrefSaver(markSaved);
	const themeLabel = useId();
	const densityLabel = useId();
	return (
		<SettingsSection
			id={id}
			title="Appearance"
			icon={<Palette className="size-4" aria-hidden />}
			description="Changes apply right away."
		>
			<SettingRow
				label="Theme"
				labelId={themeLabel}
				saved={saved === "theme"}
				description="System follows your operating system."
				control={
					<Segmented
						label="Theme"
						value={config.prefs.theme}
						options={THEMES}
						onChange={(theme) => void save("theme", { theme })}
					/>
				}
			/>
			<SettingRow
				label="Density"
				labelId={densityLabel}
				saved={saved === "density"}
				description="Compact fits more rows in token tables."
				control={
					<Segmented
						label="Density"
						value={config.prefs.density}
						options={DENSITIES}
						onChange={(density) => void save("density", { density })}
					/>
				}
			/>
			<SwitchRow
				checked={config.prefs.sidebarCollapsed}
				onChange={(sidebarCollapsed) => void save("sidebar", { sidebarCollapsed })}
				saved={saved === "sidebar"}
				label="Collapse sidebar"
				description={
					<span className="inline-flex flex-wrap items-center gap-1">
						Show icons only. Toggle anytime with <Kbd>{MOD}</Kbd>
						<Kbd>B</Kbd>
					</span>
				}
			/>
		</SettingsSection>
	);
}

/* ---- Data & privacy -------------------------------------------------------- */

type ClearKind = "watchlist" | "recent" | "history";

const CLEAR_COPY: Record<ClearKind, { title: string; description: string; confirm: string; done: string }> = {
	watchlist: {
		title: "Clear watchlist?",
		description: "Removes every token from your watchlist on this computer.",
		confirm: "Clear watchlist",
		done: "Watchlist cleared",
	},
	recent: {
		title: "Clear recent tokens?",
		description: "Empties the recent list in the command palette and Analyze.",
		confirm: "Clear recent",
		done: "Recent tokens cleared",
	},
	history: {
		title: "Clear local analysis history?",
		description:
			"Forgets which tokens you analyzed on this computer, so analyzed badges and filters reset. Your submissions on pump.studio and your XP are not affected.",
		confirm: "Clear history",
		done: "Analysis history cleared",
	},
};

export function DataSection({ id, config }: { id: string; config: PublicConfig }) {
	const analyzedCount = useApp((s) => Object.keys(s.analyzed).length);
	const toast = useApp((s) => s.toast);
	const [saved, markSaved] = useSavedFlash();
	const savePrefs = usePrefSaver(markSaved);
	const [confirm, setConfirm] = useState<ClearKind | null>(null);
	const [busy, setBusy] = useState(false);

	const watchCount = config.prefs.watchlist.length;
	const recentCount = config.prefs.recentMints.length;

	async function doClear(kind: ClearKind) {
		setBusy(true);
		const copy = CLEAR_COPY[kind];
		let ok = true;
		if (kind === "watchlist") {
			const prev = config.prefs.watchlist;
			ok = await savePrefs(kind, { watchlist: [] });
			if (ok)
				toast({
					kind: "info",
					title: copy.done,
					description: `${integer(prev.length)} ${plural(prev.length, "token")} removed`,
					action: { label: "Undo", run: () => useApp.getState().setPrefs({ watchlist: prev }) },
					duration: 6000,
				});
		} else if (kind === "recent") {
			const prev = config.prefs.recentMints;
			ok = await savePrefs(kind, { recentMints: [] });
			if (ok)
				toast({
					kind: "info",
					title: copy.done,
					action: { label: "Undo", run: () => useApp.getState().setPrefs({ recentMints: prev }) },
					duration: 6000,
				});
		} else {
			useApp.setState({ analyzed: {} });
			try {
				localStorage.removeItem(ANALYZED_STORAGE_KEY);
			} catch (err) {
				ok = false;
				toast({ kind: "error", title: "Could not clear stored history", description: String(err) });
			}
			if (ok) {
				markSaved(kind);
				toast({ kind: "info", title: copy.done, duration: 3000 });
			}
		}
		setBusy(false);
		if (ok) setConfirm(null);
	}

	return (
		<SettingsSection
			id={id}
			title="Data & privacy"
			icon={<Database className="size-4" aria-hidden />}
			description="Everything below lives only on this computer."
		>
			<SettingRow
				label="Config file"
				description="Your keys and preferences. Readable only by your user account."
				stack
				control={
					<div className="flex items-center gap-1 rounded-md border border-line bg-surface-1 py-0.5 pl-2.5 pr-0.5">
						<code className="selectable min-w-0 flex-1 truncate font-mono text-sm text-fg-2" title={config.configPath}>
							{config.configPath}
						</code>
						<CopyButton text={config.configPath} label="Copy config file path" />
					</div>
				}
			/>
			<ClearRow
				label="Watchlist"
				count={watchCount}
				unit="token"
				saved={saved === "watchlist"}
				action="Clear watchlist"
				onClick={() => setConfirm("watchlist")}
			/>
			<ClearRow
				label="Recent tokens"
				count={recentCount}
				unit="token"
				saved={saved === "recent"}
				action="Clear recent tokens"
				onClick={() => setConfirm("recent")}
			/>
			<ClearRow
				label="Local analysis history"
				count={analyzedCount}
				unit="token"
				hint="Powers the analyzed badges and filters."
				saved={saved === "history"}
				label2="Cleared"
				action="Clear my local analysis history"
				onClick={() => setConfirm("history")}
			/>
			<ConfirmDialog
				open={confirm !== null}
				onClose={() => setConfirm(null)}
				onConfirm={() => confirm && void doClear(confirm)}
				busy={busy}
				title={confirm ? CLEAR_COPY[confirm].title : ""}
				description={confirm ? CLEAR_COPY[confirm].description : ""}
				confirmLabel={confirm ? CLEAR_COPY[confirm].confirm : "Clear"}
			/>
		</SettingsSection>
	);
}

function ClearRow({
	label,
	count,
	unit,
	hint,
	saved,
	label2 = "Cleared",
	action,
	onClick,
}: {
	label: string;
	count: number;
	unit: string;
	hint?: string;
	saved: boolean;
	label2?: string;
	action: string;
	onClick: () => void;
}) {
	return (
		<div className="flex items-center gap-4 px-5 py-3.5">
			<div className="min-w-0 flex-1">
				<div className="flex items-center gap-2">
					<span className="text-base font-medium text-fg">{label}</span>
					<SavedMark show={saved} label={label2} />
				</div>
				<div className="text-sm text-fg-3">
					<span className="num">{integer(count)}</span> {plural(count, unit)}
					{hint && <> · {hint}</>}
				</div>
			</div>
			<Button size="sm" variant="ghost" className="text-neg hover:text-neg" disabled={count === 0} onClick={onClick}>
				{action}
			</Button>
		</div>
	);
}

/* ---- About ------------------------------------------------------------------- */

export function AboutSection({ id }: { id: string }) {
	const setShortcuts = useApp((s) => s.setShortcuts);
	return (
		<SettingsSection id={id} title="About" icon={<Info className="size-4" aria-hidden />}>
			<div className="flex flex-wrap items-center gap-x-4 gap-y-2 px-5 py-4">
				<div className="min-w-0 flex-1">
					<div className="flex items-center gap-2">
						<span className="text-base font-semibold text-fg">Pump.studio</span>
						<Badge>
							<span className="num">v{APP_VERSION}</span>
						</Badge>
						{isDemo && <Badge tone="warn">Demo mode</Badge>}
					</div>
					<p className="mt-0.5 text-sm text-fg-3">
						Label pump.fun tokens, submit to pump.studio and earn XP for every validated analysis.
					</p>
				</div>
				<Button size="sm" icon={<Keyboard className="size-3.5" aria-hidden />} onClick={() => setShortcuts(true)} kbd="?">
					Keyboard shortcuts
				</Button>
			</div>
			<div className="flex flex-wrap items-center gap-x-5 gap-y-2 px-5 py-3.5">
				<LinkButton href="https://pump.studio/docs">Pump.studio docs</LinkButton>
				<LinkButton href="https://github.com/tabbink2025/PumpStudio/issues">Report an issue</LinkButton>
				<span className="text-sm text-fg-3">
					Based on the open-source{" "}
					<button
						type="button"
						onClick={() => void openLink("https://github.com/pumpdotstudio/pumpstudio")}
						className="rounded font-medium text-fg-2 hover:text-fg hover:underline hover:underline-offset-2"
					>
						pumpdotstudio/pumpstudio
					</button>{" "}
					(MIT)
				</span>
			</div>
		</SettingsSection>
	);
}

/* ---- Skeleton ------------------------------------------------------------------- */

export function SectionSkeleton({ rows = 2 }: { rows?: number }) {
	return (
		<div className="rounded-lg border border-line bg-surface-2" aria-hidden>
			<div className="flex items-start gap-3 border-b border-line px-5 py-4">
				<Skeleton className="size-7 rounded-md" />
				<div className="flex-1 space-y-2 pt-0.5">
					<Skeleton className="h-3.5 w-32" />
					<Skeleton className="h-3 w-72 max-w-full" />
				</div>
			</div>
			{Array.from({ length: rows }, (_, i) => (
				<div key={i} className="flex items-center justify-between gap-4 border-b border-line px-5 py-4 last:border-b-0">
					<div className="flex-1 space-y-2">
						<Skeleton className="h-3.5 w-40" />
						<Skeleton className="h-3 w-64 max-w-full" />
					</div>
					<Skeleton className="h-8 w-40" />
				</div>
			))}
		</div>
	);
}
