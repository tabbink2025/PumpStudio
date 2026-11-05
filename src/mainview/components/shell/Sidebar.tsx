import { useEffect, useState, type ReactNode } from "react";
import {
	Bot,
	Cpu,
	ExternalLink,
	LayoutList,
	PanelLeftClose,
	PanelLeftOpen,
	Search,
	Settings as SettingsIcon,
	Sparkles,
	Trophy,
	Zap,
} from "lucide-react";
import type { ViewId } from "../../../shared/rpc-schema";
import { cn } from "../../lib/cn";
import { integer } from "../../lib/format";
import { bun, isDemo } from "../../lib/rpc";
import { useApp } from "../../lib/store";
import { TokenAvatar } from "../token/widgets";
import { IconButton } from "../ui/Button";
import { Kbd, MOD } from "../ui/primitives";
import { DragRegion, hasTrafficLights } from "./chrome";

const NAV: Array<{ id: ViewId; label: string; icon: ReactNode; key: string }> = [
	{ id: "tokens", label: "Tokens", icon: <LayoutList className="size-4" />, key: "1" },
	{ id: "analyze", label: "Analyze", icon: <Zap className="size-4" />, key: "2" },
	{ id: "auto", label: "Auto Train", icon: <Cpu className="size-4" />, key: "3" },
	{ id: "leaderboard", label: "Leaderboard", icon: <Trophy className="size-4" />, key: "4" },
	{ id: "settings", label: "Settings", icon: <SettingsIcon className="size-4" />, key: "5" },
];

function NavItem({
	active,
	collapsed,
	icon,
	label,
	onClick,
	trailing,
	title,
}: {
	active: boolean;
	collapsed: boolean;
	icon: ReactNode;
	label: string;
	onClick: () => void;
	trailing?: ReactNode;
	title?: string;
}) {
	return (
		<button
			type="button"
			onClick={onClick}
			aria-current={active ? "page" : undefined}
			title={collapsed ? label : title}
			className={cn(
				"focus-inset relative flex h-8 w-full items-center gap-2.5 rounded-md text-base transition-colors duration-fast",
				collapsed ? "justify-center px-0" : "px-2.5",
				active ? "bg-surface-3 font-medium text-fg" : "text-fg-2 hover:bg-surface-2 hover:text-fg",
			)}
		>
			{active && <span aria-hidden className="absolute inset-y-1.5 left-0 w-0.5 rounded-full bg-brand" />}
			<span className={cn("shrink-0", active ? "text-brand" : "text-fg-3")}>{icon}</span>
			{!collapsed && <span className="flex-1 truncate text-left">{label}</span>}
			{!collapsed && trailing}
			{collapsed && trailing && <span className="absolute right-1.5 top-1.5">{trailing}</span>}
		</button>
	);
}

function XpCard({ collapsed }: { collapsed: boolean }) {
	const xp = useApp((s) => s.xp);
	const flash = useApp((s) => s.xpFlash);
	const [visibleFlash, setVisibleFlash] = useState<typeof flash>();
	useEffect(() => {
		if (!flash) return;
		setVisibleFlash(flash);
		const t = setTimeout(() => setVisibleFlash(undefined), 2400);
		return () => clearTimeout(t);
	}, [flash]);

	if (collapsed) {
		return (
			<div className="flex flex-col items-center gap-0.5 py-1" title={`${integer(xp.session)} XP this session`}>
				<Sparkles className="size-4 text-brand" aria-hidden />
				<span className="num text-sm font-semibold text-fg">{xp.session > 9999 ? `${Math.round(xp.session / 1000)}k` : xp.session}</span>
			</div>
		);
	}
	return (
		<div className="relative rounded-lg border border-line bg-surface-2 p-3">
			<div className="flex items-center justify-between">
				<span className="text-caption uppercase text-fg-3">XP this session</span>
				<Sparkles className="size-3.5 text-brand" aria-hidden />
			</div>
			<div className="mt-1 flex items-baseline gap-2">
				<span className="num text-xl font-semibold text-fg">{integer(xp.session)}</span>
				{visibleFlash && (
					<span key={visibleFlash.at} className="num animate-xp-pop text-base font-semibold text-brand">
						+{visibleFlash.amount}
					</span>
				)}
			</div>
			<div className="num mt-0.5 text-sm text-fg-3">
				{xp.total != null ? `${integer(xp.total)} total` : "Total appears after your first submit"}
				{xp.submissions > 0 && ` · ${xp.validated}/${xp.submissions} validated`}
			</div>
		</div>
	);
}

function StatusLine({ collapsed }: { collapsed: boolean }) {
	const config = useApp((s) => s.config);
	const setView = useApp((s) => s.setView);
	if (!config) return null;
	const ai =
		config.orchestrator === "manual"
			? "Rule-based analysis"
			: config.orchestrator === "claude"
				? `Claude · ${config.claudeModel}`
				: `OpenAI · ${config.openaiModel}`;
	const aiReady =
		config.orchestrator === "manual" ||
		(config.orchestrator === "claude" ? config.claudeKeySet : config.openaiKeySet);

	if (collapsed) {
		return (
			<button
				type="button"
				onClick={() => setView("settings")}
				title={config.apiKeySet ? "pump.studio connected" : "No pump.studio API key"}
				className="mx-auto flex size-8 items-center justify-center rounded-md hover:bg-surface-2"
			>
				<span className={cn("size-2 rounded-full", config.apiKeySet ? "bg-pos" : "bg-warn")} />
			</button>
		);
	}
	return (
		<div className="space-y-1 px-1 text-sm">
			<button
				type="button"
				onClick={() => setView("settings")}
				className="flex w-full items-center gap-2 rounded px-1 py-0.5 text-left text-fg-2 hover:text-fg"
			>
				<span className={cn("size-2 shrink-0 rounded-full", config.apiKeySet ? "bg-pos" : "bg-warn")} aria-hidden />
				<span className="truncate">{config.apiKeySet ? "pump.studio connected" : "No API key · set up"}</span>
			</button>
			<button
				type="button"
				onClick={() => setView("settings")}
				className="flex w-full items-center gap-2 rounded px-1 py-0.5 text-left text-fg-3 hover:text-fg"
				title={aiReady ? ai : `${ai}: API key missing`}
			>
				<Bot className={cn("size-3.5 shrink-0", aiReady ? "text-violet" : "text-warn")} aria-hidden />
				<span className="truncate">{ai}</span>
			</button>
		</div>
	);
}

const EMPTY: string[] = [];

function Watchlist() {
	const watchlist = useApp((s) => s.config?.prefs.watchlist ?? EMPTY);
	const meta = useApp((s) => s.tokenMeta);
	const openAnalyze = useApp((s) => s.openAnalyze);
	if (watchlist.length === 0) return null;
	return (
		<div className="mt-5">
			<div className="mb-1 flex items-center justify-between px-2.5">
				<span className="text-caption uppercase text-fg-3">Watchlist</span>
				<span className="num text-sm text-fg-3">{watchlist.length}</span>
			</div>
			<div className="space-y-px">
				{watchlist.slice(0, 8).map((mint) => {
					const m = meta[mint];
					return (
						<button
							key={mint}
							type="button"
							onClick={() => openAnalyze(mint)}
							title={`Analyze ${m?.symbol ? `$${m.symbol}` : mint}`}
							className="focus-inset flex h-7 w-full items-center gap-2 rounded-md px-2.5 text-sm text-fg-2 hover:bg-surface-2 hover:text-fg"
						>
							<TokenAvatar mint={mint} src={m?.image} symbol={m?.symbol} size={16} />
							<span className="truncate">{m?.symbol ? `$${m.symbol}` : `${mint.slice(0, 6)}…`}</span>
						</button>
					);
				})}
			</div>
		</div>
	);
}

export function Sidebar() {
	const view = useApp((s) => s.view);
	const setView = useApp((s) => s.setView);
	const collapsed = useApp((s) => !!s.config?.prefs.sidebarCollapsed);
	const setPrefs = useApp((s) => s.setPrefs);
	const setPalette = useApp((s) => s.setPalette);
	const training = useApp((s) => s.training);
	const apiKeySet = useApp((s) => s.config?.apiKeySet ?? true);

	const running = training.status !== "idle";
	const done = training.completed + training.failed + training.skipped;

	return (
		<aside
			className={cn(
				"relative flex shrink-0 flex-col border-r border-line bg-surface-1 transition-[width] duration-base ease-out",
				collapsed ? (hasTrafficLights ? "w-[76px]" : "w-[60px]") : "w-[232px]",
			)}
			aria-label="Main navigation"
		>
			{/* Window drag strip; also clears the macOS traffic lights */}
			<div className={cn("relative shrink-0", hasTrafficLights ? "h-11" : "h-3")}>
				<DragRegion />
			</div>

			<div className={cn("flex items-center gap-2 pb-3", collapsed ? "flex-col px-2" : "px-3")}>
				<img src="./assets/pill2.png" alt="" className="size-7 shrink-0 rounded-lg" />
				{!collapsed && (
					<div className="min-w-0 flex-1 leading-tight">
						<div className="truncate text-md font-semibold text-fg">Pump.studio</div>
						<div className="text-sm text-fg-3">Agent trainer{isDemo && " · demo"}</div>
					</div>
				)}
				<IconButton
					label={collapsed ? `Expand sidebar (${MOD}B)` : `Collapse sidebar (${MOD}B)`}
					size="sm"
					onClick={() => setPrefs({ sidebarCollapsed: !collapsed })}
				>
					{collapsed ? <PanelLeftOpen className="size-4" /> : <PanelLeftClose className="size-4" />}
				</IconButton>
			</div>

			<div className={cn("pb-3", collapsed ? "px-2" : "px-3")}>
				<button
					type="button"
					onClick={() => setPalette(true)}
					title={`Search tokens, paste a mint or run a command (${MOD}K)`}
					className={cn(
						"flex h-8 w-full items-center gap-2 rounded-md border border-line-strong bg-surface-2 text-base text-fg-3 transition-colors hover:border-line-control hover:text-fg-2",
						collapsed ? "justify-center" : "px-2.5",
					)}
				>
					<Search className="size-4 shrink-0" aria-hidden />
					{!collapsed && (
						<>
							<span className="flex-1 truncate text-left">Search or paste mint</span>
							<Kbd>{MOD === "⌘" ? "⌘K" : "Ctrl K"}</Kbd>
						</>
					)}
					{collapsed && <span className="sr-only">Search</span>}
				</button>
			</div>

			<nav className={cn("min-h-0 flex-1 overflow-y-auto", collapsed ? "px-2" : "px-3")}>
				<div className="space-y-0.5">
					{NAV.map((item) => (
						<NavItem
							key={item.id}
							active={view === item.id}
							collapsed={collapsed}
							icon={item.icon}
							label={item.label}
							title={`${item.label} (${MOD}${item.key})`}
							onClick={() => setView(item.id)}
							trailing={
								item.id === "auto" && running ? (
									<span className="num inline-flex items-center gap-1 text-sm text-brand">
										<span className="size-1.5 animate-pulse rounded-full bg-brand" aria-hidden />
										{!collapsed && `${done}/${training.total}`}
									</span>
								) : item.id === "settings" && !apiKeySet ? (
									<span className="size-2 rounded-full bg-warn" aria-label="Setup needed" />
								) : null
							}
						/>
					))}
				</div>
				{!collapsed && <Watchlist />}
			</nav>

			<div className={cn("space-y-3 border-t border-line py-3", collapsed ? "px-2" : "px-3")}>
				<XpCard collapsed={collapsed} />
				<StatusLine collapsed={collapsed} />
				<button
					type="button"
					onClick={() => void bun.openWebApp("/market")}
					title="Open the pump.studio website"
					className={cn(
						"flex h-7 w-full items-center gap-2 rounded-md text-sm text-fg-3 hover:bg-surface-2 hover:text-fg",
						collapsed ? "justify-center" : "px-2",
					)}
				>
					<ExternalLink className="size-3.5 shrink-0" aria-hidden />
					{!collapsed && <span className="truncate">Open pump.studio website</span>}
				</button>
			</div>
		</aside>
	);
}
