import { useCallback, useEffect, useMemo, useState } from "react";
import { Play, Square } from "lucide-react";
import type { MarketTab, TrainingTarget } from "../../shared/types";
import { RunPanel } from "../components/auto/RunPanel";
import { SetupPanel } from "../components/auto/SetupPanel";
import { StartDialog } from "../components/auto/StartDialog";
import { EMPTY_MINTS, engineOf, retryTargets, type PickToken, type Source } from "../components/auto/model";
import { PageHeader } from "../components/shell/chrome";
import { Button } from "../components/ui/Button";
import { useKeys } from "../lib/hotkeys";
import { bun, errorMessage } from "../lib/rpc";
import { toast, useApp } from "../lib/store";

const SOURCE_KEY = "pumpstudio.auto.source";

function loadSource(): Source {
	try {
		const v = localStorage.getItem(SOURCE_KEY);
		if (v === "live" || v === "new" || v === "all" || v === "graduated" || v === "watchlist") return v;
	} catch {}
	return "live";
}

function toTarget({ mint, symbol, name, image }: PickToken): TrainingTarget {
	return { mint, symbol, name, image };
}

export function AutoTrainView({ active }: { active: boolean }) {
	const config = useApp((s) => s.config);
	const training = useApp((s) => s.training);
	const setTraining = useApp((s) => s.setTraining);
	const fetchMarket = useApp((s) => s.fetchMarket);
	const refreshTick = useApp((s) => s.refreshTick);
	const market = useApp((s) => s.market);
	const tokenMeta = useApp((s) => s.tokenMeta);
	const analyzed = useApp((s) => s.analyzed);
	const watchlist = useApp((s) => s.config?.prefs.watchlist ?? EMPTY_MINTS);

	const [source, setSourceState] = useState<Source>(loadSource);
	const [query, setQuery] = useState("");
	const [hideAnalyzed, setHideAnalyzed] = useState(false);
	const [selected, setSelected] = useState<ReadonlyMap<string, PickToken>>(() => new Map());
	const [confirm, setConfirm] = useState<{ targets: PickToken[]; retry: boolean } | null>(null);
	const [starting, setStarting] = useState(false);
	const [startError, setStartError] = useState<string>();
	const [stopping, setStopping] = useState(false);

	const setSource = useCallback((s: Source) => {
		setSourceState(s);
		try {
			localStorage.setItem(SOURCE_KEY, s);
		} catch {}
	}, []);

	/* ---- Data ------------------------------------------------------------ */

	const tab: MarketTab | null = source === "watchlist" ? null : source;
	const slice = tab ? market[tab] : undefined;

	useEffect(() => {
		if (active && tab) void fetchMarket(tab);
	}, [active, tab, fetchMarket]);

	// Global refresh (Ctrl+R / menu) while this screen is visible.
	useEffect(() => {
		if (active && refreshTick > 0 && tab) void fetchMarket(tab, { force: true });
		// eslint-disable-next-line react-hooks/exhaustive-deps
	}, [refreshTick]);

	const sourceTokens = useMemo<PickToken[]>(() => {
		if (tab) {
			return (slice?.tokens ?? []).map((t) => ({
				mint: t.mint,
				symbol: t.symbol,
				name: t.name,
				image: t.image_uri,
				mcap: t.usd_market_cap ?? t.market_cap,
			}));
		}
		// Watchlist: names from the token cache, market cap from any loaded market list.
		const caps = new Map<string, number>();
		for (const s of Object.values(market)) for (const t of s?.tokens ?? []) if (t.usd_market_cap != null) caps.set(t.mint, t.usd_market_cap);
		return watchlist.map((mint) => {
			const m = tokenMeta[mint];
			return { mint, symbol: m?.symbol, name: m?.name, image: m?.image, mcap: caps.get(mint) };
		});
	}, [tab, slice, market, watchlist, tokenMeta]);

	const visible = useMemo(() => {
		const q = query.trim().toLowerCase().replace(/^\$/, "");
		return sourceTokens.filter((t) => {
			if (hideAnalyzed && analyzed[t.mint]) return false;
			if (!q) return true;
			return (t.symbol ?? "").toLowerCase().includes(q) || (t.name ?? "").toLowerCase().includes(q) || t.mint.toLowerCase().startsWith(q);
		});
	}, [sourceTokens, query, hideAnalyzed, analyzed]);

	const visibleSet = useMemo(() => new Set(visible.map((t) => t.mint)), [visible]);
	const hiddenSelected = useMemo(() => {
		let n = 0;
		for (const m of selected.keys()) if (!visibleSet.has(m)) n++;
		return n;
	}, [selected, visibleSet]);

	/* ---- Selection --------------------------------------------------------- */

	const onToggle = useCallback((t: PickToken) => {
		setSelected((prev) => {
			const next = new Map(prev);
			if (next.has(t.mint)) next.delete(t.mint);
			else next.set(t.mint, t);
			return next;
		});
	}, []);

	const onSelectAll = () =>
		setSelected((prev) => {
			const next = new Map(prev);
			for (const t of visible) if (!next.has(t.mint)) next.set(t.mint, t);
			return next;
		});
	const onSelectNone = () =>
		setSelected((prev) => {
			const next = new Map(prev);
			for (const t of visible) next.delete(t.mint);
			return next;
		});
	const onRemoveHidden = () =>
		setSelected((prev) => {
			const next = new Map<string, PickToken>();
			for (const [m, t] of prev) if (visibleSet.has(m)) next.set(m, t);
			return next;
		});

	/* ---- Run control ----------------------------------------------------------- */

	const running = training.status !== "idle";
	const engine = engineOf(config);
	const blocked = !config
		? "Loading settings…"
		: !config.apiKeySet
			? "Add your pump.studio API key in Settings to start."
			: selected.size === 0
				? "Select at least one token to start."
				: undefined;

	function openConfirm(targets: PickToken[], retry = false) {
		setStartError(undefined);
		setConfirm({ targets, retry });
	}

	async function start() {
		if (!confirm) return;
		setStarting(true);
		setStartError(undefined);
		try {
			const state = await bun.startAutoTraining({ targets: confirm.targets.map(toTarget) });
			setTraining(state);
			setConfirm(null);
		} catch (err) {
			setStartError(errorMessage(err));
		} finally {
			setStarting(false);
		}
	}

	async function stop() {
		setStopping(true);
		try {
			setTraining(await bun.stopAutoTraining());
		} catch (err) {
			toast({ kind: "error", title: "Could not stop training", description: errorMessage(err) });
		} finally {
			setStopping(false);
		}
	}

	const retry = () => openConfirm(retryTargets(training), true);
	const closeConfirm = useCallback(() => setConfirm(null), []);

	useKeys((e) => {
		if (e.key.toLowerCase() === "s" && !running && !blocked) {
			openConfirm([...selected.values()]);
			return true;
		}
	}, active);

	const actions =
		training.status === "stopping" ? (
			<Button variant="danger" disabled icon={<Square className="size-3.5" />}>
				Stopping…
			</Button>
		) : running ? (
			<Button variant="danger" loading={stopping} icon={<Square className="size-3.5" fill="currentColor" />} onClick={() => void stop()}>
				Stop
			</Button>
		) : (
			<Button
				variant="primary"
				icon={<Play className="size-4" />}
				kbd="S"
				disabled={!!blocked}
				title={blocked}
				onClick={() => openConfirm([...selected.values()])}
			>
				{selected.size > 0 ? `Start training (${selected.size})` : "Start training"}
			</Button>
		);

	return (
		<div className="flex min-h-0 flex-1 flex-col">
			<PageHeader title="Auto Train" description="Let your agent analyze a batch of tokens and submit them for XP" actions={actions} />
			<div className="flex min-h-0 flex-1">
				<SetupPanel
					source={source}
					onSource={setSource}
					query={query}
					onQuery={setQuery}
					hideAnalyzed={hideAnalyzed}
					onHideAnalyzed={setHideAnalyzed}
					visible={visible}
					sourceCount={sourceTokens.length}
					loading={!!slice?.loading || (!!tab && !slice)}
					error={slice?.error}
					onRetry={() => tab && void fetchMarket(tab, { force: true })}
					selected={selected}
					hiddenSelected={hiddenSelected}
					onToggle={onToggle}
					onSelectAll={onSelectAll}
					onSelectNone={onSelectNone}
					onRemoveHidden={onRemoveHidden}
					config={config}
					blocked={blocked}
					running={running}
				/>
				<RunPanel
					t={training}
					onRetry={retry}
					canRetry={!!config?.apiKeySet && !running}
					engineLabel={training.orchestrator === config?.orchestrator ? engine.label : training.orchestrator === "manual" ? "Rules" : training.orchestrator === "claude" ? "Claude" : "OpenAI"}
				/>
			</div>
			<StartDialog
				targets={confirm?.targets ?? null}
				retry={confirm?.retry}
				config={config}
				starting={starting}
				error={startError}
				onClose={closeConfirm}
				onConfirm={() => void start()}
			/>
		</div>
	);
}
