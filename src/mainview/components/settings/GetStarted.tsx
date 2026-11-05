import { useEffect, useState } from "react";
import { ArrowRight, Check, X } from "lucide-react";
import type { PublicConfig } from "../../../shared/types";
import { cn } from "../../lib/cn";
import { integer } from "../../lib/format";
import { useApp } from "../../lib/store";
import { Button, IconButton } from "../ui/Button";
import { AI_CHOSEN_EVENT, AI_CHOSEN_KEY } from "./AiSection";

const HIDDEN_KEY = "pumpstudio.settings.getStartedHidden";

function readFlag(key: string): boolean {
	try {
		return localStorage.getItem(key) === "1";
	} catch {
		return false;
	}
}

export interface Step {
	title: string;
	description: string;
	done: boolean;
	action?: { label: string; run: () => void };
}

/** "Get started" checklist, shown while setup is incomplete. */
export function GetStarted({ config, onJump }: { config: PublicConfig; onJump: (section: "account" | "ai") => void }) {
	const submissions = useApp((s) => s.xp.submissions);
	const xpTotal = useApp((s) => s.xp.total ?? 0);
	const hasHistory = useApp((s) => Object.keys(s.analyzed).length > 0);
	const setView = useApp((s) => s.setView);
	const [hidden, setHidden] = useState(() => readFlag(HIDDEN_KEY));
	const [chosenFlag, setChosenFlag] = useState(() => readFlag(AI_CHOSEN_KEY));
	useEffect(() => {
		const on = () => setChosenFlag(true);
		window.addEventListener(AI_CHOSEN_EVENT, on);
		return () => window.removeEventListener(AI_CHOSEN_EVENT, on);
	}, []);

	const aiChosen =
		config.orchestrator !== "manual" || config.claudeKeySet || config.openaiKeySet || chosenFlag;
	const submitted = submissions > 0 || xpTotal > 0 || hasHistory;

	const steps: Step[] = [
		{
			title: "Connect pump.studio",
			description: "Paste your agent key or create one, so submissions earn XP.",
			done: config.apiKeySet,
			action: { label: "Add key", run: () => onJump("account") },
		},
		{
			title: "Choose how analyses are filled",
			description: "Data rules are free. Claude or OpenAI write richer summaries.",
			done: aiChosen,
			action: { label: "Choose", run: () => onJump("ai") },
		},
		{
			title: "Submit your first analysis",
			description: "Pick a token, review the fields and submit.",
			done: submitted,
			action: { label: "Browse tokens", run: () => setView("tokens") },
		},
	];
	const doneCount = steps.filter((s) => s.done).length;
	const complete = config.apiKeySet && submitted;
	if (complete || hidden) return null;
	const nextIdx = steps.findIndex((s) => !s.done);

	function hide() {
		setHidden(true);
		try {
			localStorage.setItem(HIDDEN_KEY, "1");
		} catch {}
	}

	return (
		<section aria-labelledby="get-started-title" className="rounded-lg border border-brand/25 bg-surface-2 shadow-card">
			<header className="flex items-center gap-3 px-5 pb-2 pt-4">
				<div className="min-w-0 flex-1">
					<h2 id="get-started-title" className="text-md font-semibold text-fg">
						Get started
					</h2>
					<p className="mt-0.5 text-base text-fg-3">Three steps to your first XP.</p>
				</div>
				<div
					className="flex items-center gap-2"
					role="progressbar"
					aria-label="Setup progress"
					aria-valuemin={0}
					aria-valuemax={steps.length}
					aria-valuenow={doneCount}
					aria-valuetext={`${doneCount} of ${steps.length} steps done`}
				>
					<div className="flex gap-1" aria-hidden>
						{steps.map((_, i) => (
							<span key={i} className={cn("h-1.5 w-6 rounded-full", i < doneCount ? "bg-pos" : "bg-surface-4")} />
						))}
					</div>
					<span className="num text-sm text-fg-3">
						{integer(doneCount)}/{steps.length}
					</span>
				</div>
				<IconButton label="Hide checklist" size="sm" onClick={hide}>
					<X className="size-3.5" aria-hidden />
				</IconButton>
			</header>
			<ol className="px-2 pb-2">
				{steps.map((s, i) => {
					const isNext = i === nextIdx;
					return (
						<li
							key={s.title}
							className={cn("flex items-center gap-3 rounded-md px-3 py-2.5", isNext && "bg-surface-3/60")}
						>
							<span
								className={cn(
									"num flex size-6 shrink-0 items-center justify-center rounded-full border text-sm font-semibold",
									s.done
										? "border-pos/30 bg-pos/10 text-pos"
										: isNext
											? "border-brand/50 text-brand"
											: "border-line-strong text-fg-3",
								)}
								aria-hidden
							>
								{s.done ? <Check className="size-3.5" strokeWidth={3} /> : i + 1}
							</span>
							<div className="min-w-0 flex-1">
								<div className={cn("text-base font-medium", s.done ? "text-fg-3 line-through decoration-fg-3/50" : "text-fg")}>
									{s.title}
									<span className="sr-only">{s.done ? " (done)" : " (to do)"}</span>
								</div>
								{!s.done && <div className="text-sm text-fg-3">{s.description}</div>}
							</div>
							{s.done ? (
								<span className="text-sm font-medium text-pos">Done</span>
							) : (
								s.action && (
									<Button
										size="sm"
										variant={isNext ? "primary" : "ghost"}
										iconRight={<ArrowRight className="size-3.5" aria-hidden />}
										onClick={s.action.run}
									>
										{s.action.label}
									</Button>
								)
							)}
						</li>
					);
				})}
			</ol>
		</section>
	);
}
