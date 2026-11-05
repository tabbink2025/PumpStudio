import { useApp } from "../../lib/store";
import { Dialog } from "../ui/overlays";
import { Kbd, MOD } from "../ui/primitives";

const GROUPS: Array<{ title: string; items: Array<[string, string[]]> }> = [
	{
		title: "Everywhere",
		items: [
			["Search tokens or run a command", [MOD, "K"]],
			["Search (alternative)", ["/"]],
			["Go to Tokens / Analyze / Auto Train / Leaderboard / Settings", [MOD, "1–5"]],
			["Go to a screen (then t, a, r, l or s)", ["G"]],
			["Toggle sidebar", [MOD, "B"]],
			["Refresh data", [MOD, "R"]],
			["Show this list", ["?"]],
		],
	},
	{
		title: "Tokens",
		items: [
			["Move selection", ["J", "K"]],
			["Open details", ["Enter"]],
			["Analyze selected token", ["A"]],
			["Add to / remove from watchlist", ["W"]],
			["Copy mint address", ["C"]],
			["Close details", ["Esc"]],
		],
	},
	{
		title: "Auto Train",
		items: [["Start a training run (asks to confirm)", ["S"]]],
	},
	{
		title: "Analyze",
		items: [
			["Submit analysis", [MOD, "Enter"]],
			["Auto-fill with AI", [MOD, "I"]],
		],
	},
];

export function ShortcutsDialog() {
	const open = useApp((s) => s.shortcutsOpen);
	const setOpen = useApp((s) => s.setShortcuts);
	return (
		<Dialog open={open} onClose={() => setOpen(false)} title="Keyboard shortcuts" labelledBy="shortcuts-title" width={560}>
			<div className="space-y-5">
				{GROUPS.map((g) => (
					<section key={g.title}>
						<h3 className="mb-2 text-caption uppercase text-fg-3">{g.title}</h3>
						<dl className="divide-y divide-line">
							{g.items.map(([label, keys]) => (
								<div key={label} className="flex items-center justify-between gap-4 py-1.5">
									<dt className="text-base text-fg-2">{label}</dt>
									<dd className="flex shrink-0 gap-1">
										{keys.map((k) => (
											<Kbd key={k}>{k}</Kbd>
										))}
									</dd>
								</div>
							))}
						</dl>
					</section>
				))}
			</div>
		</Dialog>
	);
}
