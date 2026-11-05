import { useEffect, useMemo, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { Command } from "cmdk";
import {
	Cpu,
	ExternalLink,
	Keyboard,
	LayoutList,
	Moon,
	PanelLeft,
	RefreshCw,
	Search,
	Settings,
	Sun,
	Trophy,
	Zap,
} from "lucide-react";
import { extractMint, shortMint } from "../../../shared/analysis";
import type { ViewId } from "../../../shared/rpc-schema";
import { bun } from "../../lib/rpc";
import { useApp } from "../../lib/store";
import { TokenAvatar } from "../token/widgets";
import { Kbd, MOD } from "../ui/primitives";

function Item({
	value,
	onSelect,
	icon,
	children,
	hint,
	keywords,
}: {
	value: string;
	onSelect: () => void;
	icon?: ReactNode;
	children: ReactNode;
	hint?: ReactNode;
	keywords?: string[];
}) {
	return (
		<Command.Item
			value={value}
			keywords={keywords}
			onSelect={onSelect}
			className="flex h-9 cursor-default items-center gap-3 rounded-md px-2.5 text-base text-fg-2 data-[selected=true]:bg-surface-3 data-[selected=true]:text-fg"
		>
			<span className="flex size-5 shrink-0 items-center justify-center text-fg-3">{icon}</span>
			<span className="min-w-0 flex-1 truncate">{children}</span>
			{hint && <span className="shrink-0 text-sm text-fg-3">{hint}</span>}
		</Command.Item>
	);
}

const GROUP =
	"px-1.5 pb-1 [&_[cmdk-group-heading]]:px-2.5 [&_[cmdk-group-heading]]:pb-1 [&_[cmdk-group-heading]]:pt-2 [&_[cmdk-group-heading]]:text-caption [&_[cmdk-group-heading]]:uppercase [&_[cmdk-group-heading]]:text-fg-3";

export function CommandPalette() {
	const open = useApp((s) => s.paletteOpen);
	const setOpen = useApp((s) => s.setPalette);
	const setView = useApp((s) => s.setView);
	const openAnalyze = useApp((s) => s.openAnalyze);
	const refresh = useApp((s) => s.refresh);
	const setPrefs = useApp((s) => s.setPrefs);
	const setShortcuts = useApp((s) => s.setShortcuts);
	const config = useApp((s) => s.config);
	const tokenMeta = useApp((s) => s.tokenMeta);
	const [query, setQuery] = useState("");

	useEffect(() => {
		if (open) setQuery("");
	}, [open]);

	const pastedMint = useMemo(() => extractMint(query), [query]);

	const tokens = useMemo(() => {
		const recent = config?.prefs.recentMints ?? [];
		const watch = config?.prefs.watchlist ?? [];
		const seen = new Set<string>();
		const ordered: string[] = [];
		for (const m of [...recent, ...watch, ...Object.keys(tokenMeta)]) {
			if (!seen.has(m)) {
				seen.add(m);
				ordered.push(m);
			}
		}
		return ordered.slice(0, 300).map((mint) => ({ mint, ...tokenMeta[mint], recent: recent.includes(mint), watched: watch.includes(mint) }));
	}, [config?.prefs.recentMints, config?.prefs.watchlist, tokenMeta]);

	if (!open) return null;

	const go = (v: ViewId) => {
		setView(v);
		setOpen(false);
	};
	const run = (fn: () => void) => () => {
		fn();
		setOpen(false);
	};
	const theme = config?.prefs.theme ?? "system";
	const nextTheme = theme === "dark" ? "light" : "dark";

	return createPortal(
		<div className="fixed inset-0 z-50 flex items-start justify-center bg-black/50 px-4 pt-[14vh] animate-fade-in" onMouseDown={() => setOpen(false)}>
			<div onMouseDown={(e) => e.stopPropagation()} className="w-[640px] max-w-full animate-pop-in">
				<Command
					label="Command palette"
					loop
					onKeyDown={(e) => {
						if (e.key === "Escape") {
							e.preventDefault();
							setOpen(false);
						}
					}}
					className="overflow-hidden rounded-xl border border-line-strong bg-surface-2 shadow-pop"
				>
					<div className="flex items-center gap-3 border-b border-line px-4">
						<Search className="size-4 shrink-0 text-fg-3" aria-hidden />
						<Command.Input
							autoFocus
							value={query}
							onValueChange={setQuery}
							placeholder="Search tokens, paste a mint or URL, or type a command…"
							className="h-12 flex-1 bg-transparent text-md text-fg outline-none placeholder:text-fg-3"
						/>
						<Kbd>Esc</Kbd>
					</div>
					<Command.List className="max-h-[420px] overflow-y-auto py-1.5">
						<Command.Empty className="px-4 py-8 text-center text-base text-fg-3">
							No matches. Paste a full mint address to analyze any token.
						</Command.Empty>

						{pastedMint && (
							<Command.Group heading="Mint address" className={GROUP} forceMount>
								<Item
									value={`analyze-mint-${pastedMint}`}
									keywords={[query]}
									icon={<Zap className="size-4 text-brand" />}
									onSelect={run(() => openAnalyze(pastedMint))}
									hint="Enter"
								>
									Analyze <span className="font-mono text-sm">{shortMint(pastedMint, 6, 6)}</span>
								</Item>
							</Command.Group>
						)}

						<Command.Group heading="Go to" className={GROUP}>
							<Item value="Tokens" icon={<LayoutList className="size-4" />} onSelect={() => go("tokens")} hint={`${MOD}1`}>
								Tokens
							</Item>
							<Item value="Analyze" icon={<Zap className="size-4" />} onSelect={() => go("analyze")} hint={`${MOD}2`}>
								Analyze
							</Item>
							<Item value="Auto Train" keywords={["training", "bot"]} icon={<Cpu className="size-4" />} onSelect={() => go("auto")} hint={`${MOD}3`}>
								Auto Train
							</Item>
							<Item value="Leaderboard" icon={<Trophy className="size-4" />} onSelect={() => go("leaderboard")} hint={`${MOD}4`}>
								Leaderboard
							</Item>
							<Item value="Settings" keywords={["api key", "preferences"]} icon={<Settings className="size-4" />} onSelect={() => go("settings")} hint={`${MOD},`}>
								Settings
							</Item>
						</Command.Group>

						{tokens.length > 0 && (
							<Command.Group heading="Tokens" className={GROUP}>
								{tokens.map((t) => (
									<Item
										key={t.mint}
										value={`token-${t.mint}`}
										keywords={[t.symbol ?? "", t.name ?? "", t.mint]}
										icon={<TokenAvatar mint={t.mint} src={t.image} symbol={t.symbol} size={20} />}
										onSelect={run(() => openAnalyze(t.mint))}
										hint={t.recent ? "Recent" : t.watched ? "Watching" : undefined}
									>
										<span className="font-medium text-fg">{t.symbol ? `$${t.symbol}` : shortMint(t.mint)}</span>
										{t.name && <span className="ml-2 text-fg-3">{t.name}</span>}
									</Item>
								))}
							</Command.Group>
						)}

						<Command.Group heading="Actions" className={GROUP}>
							<Item value="Refresh data" icon={<RefreshCw className="size-4" />} onSelect={run(refresh)} hint={`${MOD}R`}>
								Refresh data
							</Item>
							<Item
								value="Toggle sidebar"
								icon={<PanelLeft className="size-4" />}
								onSelect={run(() => setPrefs({ sidebarCollapsed: !config?.prefs.sidebarCollapsed }))}
								hint={`${MOD}B`}
							>
								Toggle sidebar
							</Item>
							<Item
								value={`Switch to ${nextTheme} theme`}
								keywords={["theme", "dark", "light", "appearance"]}
								icon={nextTheme === "dark" ? <Moon className="size-4" /> : <Sun className="size-4" />}
								onSelect={run(() => setPrefs({ theme: nextTheme }))}
							>
								Switch to {nextTheme} theme
							</Item>
							<Item
								value="Toggle density"
								keywords={["compact", "comfortable", "rows"]}
								icon={<LayoutList className="size-4" />}
								onSelect={run(() => setPrefs({ density: config?.prefs.density === "compact" ? "comfortable" : "compact" }))}
							>
								Use {config?.prefs.density === "compact" ? "comfortable" : "compact"} rows
							</Item>
							<Item value="Keyboard shortcuts" icon={<Keyboard className="size-4" />} onSelect={run(() => setShortcuts(true))} hint="?">
								Keyboard shortcuts
							</Item>
							<Item value="Open pump.studio website" icon={<ExternalLink className="size-4" />} onSelect={run(() => void bun.openWebApp("/market"))}>
								Open pump.studio website
							</Item>
						</Command.Group>
					</Command.List>
				</Command>
			</div>
		</div>,
		document.body,
	);
}
