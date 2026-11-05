import { useEffect, useRef, useState } from "react";

/** Current time, re-rendering every `ms` while `enabled`. */
export function useNow(ms: number, enabled = true): number {
	const [now, setNow] = useState(() => Date.now());
	useEffect(() => {
		if (!enabled) return;
		setNow(Date.now());
		const t = setInterval(() => setNow(Date.now()), ms);
		return () => clearInterval(t);
	}, [ms, enabled]);
	return now;
}

/** Width of an element, tracked with ResizeObserver. */
export function useWidth<T extends HTMLElement>() {
	const ref = useRef<T>(null);
	const [width, setWidth] = useState(0);
	useEffect(() => {
		const el = ref.current;
		if (!el) return;
		setWidth(el.getBoundingClientRect().width);
		const ro = new ResizeObserver((entries) => {
			const w = entries[0]?.contentRect.width;
			if (w != null) setWidth(w);
		});
		ro.observe(el);
		return () => ro.disconnect();
	}, []);
	return [ref, width] as const;
}
