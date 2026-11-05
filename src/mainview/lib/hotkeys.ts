import { useEffect, useRef } from "react";

/** True when the event comes from a text field (shortcuts must not fire there). */
export function isTypingTarget(e: KeyboardEvent): boolean {
	const el = e.target as HTMLElement | null;
	if (!el) return false;
	const tag = el.tagName;
	return tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || el.isContentEditable;
}

/** Any open modal/palette blocks screen-level shortcuts. */
export function overlayOpen(): boolean {
	return !!document.querySelector('[role="dialog"][aria-modal="true"], [cmdk-root]');
}

export const isModKey = (e: KeyboardEvent | React.KeyboardEvent) => e.metaKey || e.ctrlKey;

/**
 * Screen-level single-key shortcuts (j, k, w, Enter...). Ignored while typing,
 * with modifiers held, or when a dialog/palette is open.
 * The handler returns true when it handled the key (to preventDefault).
 */
export function useKeys(handler: (e: KeyboardEvent) => boolean | void, enabled = true) {
	const ref = useRef(handler);
	ref.current = handler;
	useEffect(() => {
		if (!enabled) return;
		const onKey = (e: KeyboardEvent) => {
			if (e.defaultPrevented || isTypingTarget(e) || overlayOpen()) return;
			if (e.metaKey || e.ctrlKey || e.altKey) return;
			if (ref.current(e) === true) e.preventDefault();
		};
		window.addEventListener("keydown", onKey);
		return () => window.removeEventListener("keydown", onKey);
	}, [enabled]);
}
