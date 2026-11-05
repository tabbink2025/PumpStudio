import { useEffect, useState, type ReactNode, type RefObject } from "react";
import { cn } from "../../lib/cn";

export interface NavSection {
	id: string;
	label: string;
	icon: ReactNode;
	/** Small marker next to the label, e.g. a "needs attention" dot. */
	marker?: ReactNode;
}

function prefersReducedMotion() {
	return typeof window !== "undefined" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

export function scrollToSection(id: string, focus = true) {
	const el = document.getElementById(id);
	if (!el) return;
	el.scrollIntoView({ behavior: prefersReducedMotion() ? "auto" : "smooth", block: "start" });
	if (focus) {
		const heading = el.querySelector<HTMLElement>("h2");
		if (heading) {
			heading.tabIndex = -1;
			heading.focus({ preventScroll: true });
		}
	}
}

/** Tracks which section is at the top of the scroll container. */
export function useActiveSection(sections: readonly NavSection[], root: RefObject<HTMLElement>, enabled: boolean) {
	const [activeId, setActiveId] = useState(sections[0]?.id ?? "");
	useEffect(() => {
		const container = root.current;
		if (!enabled || !container) return;
		const update = () => {
			const top = container.getBoundingClientRect().top;
			let current = sections[0]?.id ?? "";
			for (const s of sections) {
				const el = document.getElementById(s.id);
				if (!el) continue;
				if (el.getBoundingClientRect().top - top <= 96) current = s.id;
			}
			// At the very bottom the last short sections can never reach the top.
			if (container.scrollTop + container.clientHeight >= container.scrollHeight - 4) {
				current = sections[sections.length - 1]?.id ?? current;
			}
			setActiveId(current);
		};
		update();
		container.addEventListener("scroll", update, { passive: true });
		window.addEventListener("resize", update);
		return () => {
			container.removeEventListener("scroll", update);
			window.removeEventListener("resize", update);
		};
	}, [sections, root, enabled]);
	return [activeId, setActiveId] as const;
}

export function SectionNav({
	sections,
	activeId,
	onSelect,
}: {
	sections: readonly NavSection[];
	activeId: string;
	onSelect: (id: string) => void;
}) {
	return (
		<nav aria-label="Settings sections">
			<ul className="space-y-0.5">
				{sections.map((s) => {
					const active = s.id === activeId;
					return (
						<li key={s.id}>
							<a
								href={`#${s.id}`}
								aria-current={active ? "true" : undefined}
								onClick={(e) => {
									e.preventDefault();
									onSelect(s.id);
								}}
								className={cn(
									"focus-inset relative flex h-8 items-center gap-2.5 rounded-md px-2.5 text-base transition-colors duration-fast",
									active ? "bg-surface-3 font-medium text-fg" : "text-fg-2 hover:bg-surface-2 hover:text-fg",
								)}
							>
								{active && <span aria-hidden className="absolute inset-y-1.5 left-0 w-0.5 rounded-full bg-brand" />}
								<span className={cn("shrink-0", active ? "text-brand" : "text-fg-3")}>{s.icon}</span>
								<span className="min-w-0 flex-1 truncate">{s.label}</span>
								{s.marker}
							</a>
						</li>
					);
				})}
			</ul>
		</nav>
	);
}
