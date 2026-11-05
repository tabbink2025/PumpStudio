import { forwardRef, useEffect, useId, useState, type ReactNode } from "react";
import { Check, Search, X } from "lucide-react";
import { cn } from "../../lib/cn";
import { absoluteTime, ago } from "../../lib/format";

/** Re-render on an interval (for relative times). */
export function useNow(intervalMs: number, enabled = true): number {
	const [now, setNow] = useState(() => Date.now());
	useEffect(() => {
		if (!enabled) return;
		setNow(Date.now());
		const id = setInterval(() => setNow(Date.now()), intervalMs);
		return () => clearInterval(id);
	}, [intervalMs, enabled]);
	return now;
}

/** "Updated 12s ago", ticking every 5s while the screen is visible. */
export function UpdatedAgo({ at, loading, active }: { at?: number; loading: boolean; active: boolean }) {
	const now = useNow(5000, active);
	const text = loading && !at ? "Loading" : loading ? "Updating" : at ? `Updated ${ago(at, Math.max(now, at))}` : "Not loaded yet";
	return (
		<span className="num hidden whitespace-nowrap text-sm text-fg-3 lg:inline" title={at ? absoluteTime(at) : undefined}>
			{text}
		</span>
	);
}

/** Toggle chip for list filters (aria-pressed, check icon when on). */
export function FilterChip({
	pressed,
	onToggle,
	icon,
	children,
	title,
}: {
	pressed: boolean;
	onToggle: () => void;
	icon: ReactNode;
	children: ReactNode;
	title?: string;
}) {
	return (
		<button
			type="button"
			aria-pressed={pressed}
			title={title}
			onClick={onToggle}
			className={cn(
				"inline-flex h-7 shrink-0 items-center gap-1.5 whitespace-nowrap rounded-md border px-2.5 text-sm font-medium transition-colors duration-fast",
				pressed
					? "border-brand/30 bg-brand/10 text-brand hover:bg-brand/15"
					: "border-line-strong bg-surface-1 text-fg-2 hover:border-line-control hover:text-fg",
			)}
		>
			<span className="flex size-3.5 items-center justify-center" aria-hidden>
				{pressed ? <Check className="size-3.5" /> : icon}
			</span>
			{children}
		</button>
	);
}

/** Filter input with a leading icon and a clear button. */
export const FilterInput = forwardRef<
	HTMLInputElement,
	{
		value: string;
		onChange: (v: string) => void;
		onEnter?: () => void;
		className?: string;
	}
>(function FilterInput({ value, onChange, onEnter, className }, ref) {
	const id = useId();
	return (
		<div className={cn("field relative flex h-7 items-center", className)}>
			<label htmlFor={id} className="sr-only">
				Filter tokens
			</label>
			<Search className="pointer-events-none absolute left-2 size-3.5 text-fg-3" aria-hidden />
			<input
				ref={ref}
				id={id}
				type="text"
				value={value}
				spellCheck={false}
				autoComplete="off"
				placeholder="Filter by name, ticker or mint"
				onChange={(e) => onChange(e.target.value)}
				onKeyDown={(e) => {
					if (e.key === "Escape") {
						e.preventDefault();
						e.stopPropagation();
						// First Escape clears, the next one hands the keyboard back to the list.
						if (value) onChange("");
						else e.currentTarget.blur();
					}
					if (e.key === "Enter" && onEnter) {
						e.preventDefault();
						onEnter();
					}
				}}
				className="h-full w-full min-w-0 bg-transparent pl-7 pr-7 text-sm text-fg outline-none placeholder:text-fg-3"
			/>
			{value && (
				<button
					type="button"
					aria-label="Clear filter"
					title="Clear filter"
					onClick={() => onChange("")}
					className="absolute right-1 inline-flex size-5 items-center justify-center rounded text-fg-3 hover:bg-surface-3 hover:text-fg"
				>
					<X className="size-3.5" aria-hidden />
				</button>
			)}
		</div>
	);
});
