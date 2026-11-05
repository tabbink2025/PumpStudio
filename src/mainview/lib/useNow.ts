import { useEffect, useState } from "react";

/** Current time, re-rendered every `ms` while `enabled` (for "12s ago" labels). */
export function useNow(ms = 5000, enabled = true): number {
	const [now, setNow] = useState(() => Date.now());
	useEffect(() => {
		if (!enabled) return;
		setNow(Date.now());
		const t = setInterval(() => setNow(Date.now()), ms);
		return () => clearInterval(t);
	}, [ms, enabled]);
	return now;
}
