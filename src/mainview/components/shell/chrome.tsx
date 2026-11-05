import type { ReactNode } from "react";
import { KeyRound } from "lucide-react";
import { cn } from "../../lib/cn";
import { isDesktop } from "../../lib/rpc";
import { useApp } from "../../lib/store";
import { Button } from "../ui/Button";
import { isMac } from "../ui/primitives";

/** True when macOS traffic lights overlay the top-left of the window (hiddenInset). */
export const hasTrafficLights = isDesktop && isMac;

/**
 * An empty layer that drags the window. Electrobun has no "no-drag" opt-out:
 * a mousedown anywhere inside a drag element moves the window. So this sits
 * behind the content as a sibling, and the content layer above it is
 * pointer-events: none except for its interactive children.
 */
export function DragRegion({ className }: { className?: string }) {
	if (!isDesktop) return null;
	return <div aria-hidden className={cn("electrobun-webkit-app-region-drag absolute inset-0", className)} />;
}

/**
 * Top bar of every screen: title on the left, actions on the right.
 * Empty space drags the window.
 */
export function PageHeader({
	title,
	description,
	actions,
	children,
	className,
}: {
	title: ReactNode;
	description?: ReactNode;
	actions?: ReactNode;
	/** Optional second row (tabs, filters). */
	children?: ReactNode;
	className?: string;
}) {
	return (
		<header className={cn("relative shrink-0 border-b border-line bg-bg", className)}>
			<DragRegion />
			<div
				className="pointer-events-none relative flex h-[52px] items-center gap-3 px-5 [&_a]:pointer-events-auto [&_button]:pointer-events-auto [&_input]:pointer-events-auto"
			>
				<div className="flex min-w-0 flex-1 items-baseline gap-3">
					<h1 className="truncate text-lg font-semibold text-fg">{title}</h1>
					{description && <p className="hidden truncate text-base text-fg-3 md:block">{description}</p>}
				</div>
				{actions && <div className="flex shrink-0 items-center gap-2">{actions}</div>}
			</div>
			{children && (
				<div className="pointer-events-none relative px-5 pb-3 [&_button]:pointer-events-auto [&_input]:pointer-events-auto [&_label]:pointer-events-auto [&_select]:pointer-events-auto">
					{children}
				</div>
			)}
		</header>
	);
}

/** Prompt to connect pump.studio, shown where an API key is required. */
export function SetupBanner({ className, compact }: { className?: string; compact?: boolean }) {
	const config = useApp((s) => s.config);
	const setView = useApp((s) => s.setView);
	if (!config || config.apiKeySet) return null;
	return (
		<div
			className={cn(
				"flex items-center gap-3 rounded-lg border border-brand/30 bg-brand/5 px-3 py-2.5",
				compact ? "text-sm" : "text-base",
				className,
			)}
		>
			<KeyRound className="size-4 shrink-0 text-brand" aria-hidden />
			<div className="min-w-0 flex-1">
				<span className="font-medium text-fg">Connect your pump.studio agent key</span>
				<span className="text-fg-2"> to submit analyses and earn XP. Browsing works without one.</span>
			</div>
			<Button size="sm" variant="primary" onClick={() => setView("settings")}>
				Set up
			</Button>
		</div>
	);
}
