import { useCallback, useMemo, useRef } from "react";
import { Cpu, Database, Info, Palette, Sparkles, User } from "lucide-react";
import { PageHeader } from "../components/shell/chrome";
import { AccountSection } from "../components/settings/AccountSection";
import { AiSection } from "../components/settings/AiSection";
import { GetStarted } from "../components/settings/GetStarted";
import { scrollToSection, SectionNav, useActiveSection, type NavSection } from "../components/settings/SectionNav";
import {
	AboutSection,
	AppearanceSection,
	AutoTrainSection,
	DataSection,
	SectionSkeleton,
} from "../components/settings/sections";
import { ErrorState } from "../components/ui/primitives";
import { useApp } from "../lib/store";

const IDS = {
	account: "settings-account",
	ai: "settings-ai",
	auto: "settings-auto",
	appearance: "settings-appearance",
	data: "settings-data",
	about: "settings-about",
} as const;

function Dot({ label }: { label: string }) {
	return (
		<span className="size-1.5 shrink-0 rounded-full bg-warn" title={label}>
			<span className="sr-only">{label}</span>
		</span>
	);
}

export function SettingsView({ active }: { active: boolean }) {
	const config = useApp((s) => s.config);
	const configError = useApp((s) => s.configError);
	const loadConfig = useApp((s) => s.loadConfig);
	const scroller = useRef<HTMLDivElement>(null);

	const apiMissing = config ? !config.apiKeySet : false;
	const aiMissing = config
		? (config.orchestrator === "claude" && !config.claudeKeySet) || (config.orchestrator === "openai" && !config.openaiKeySet)
		: false;

	const sections = useMemo<NavSection[]>(
		() => [
			{
				id: IDS.account,
				label: "Account",
				icon: <User className="size-4" aria-hidden />,
				marker: apiMissing ? <Dot label="No API key" /> : undefined,
			},
			{
				id: IDS.ai,
				label: "AI analysis",
				icon: <Sparkles className="size-4" aria-hidden />,
				marker: aiMissing ? <Dot label="AI key missing" /> : undefined,
			},
			{ id: IDS.auto, label: "Auto train", icon: <Cpu className="size-4" aria-hidden /> },
			{ id: IDS.appearance, label: "Appearance", icon: <Palette className="size-4" aria-hidden /> },
			{ id: IDS.data, label: "Data & privacy", icon: <Database className="size-4" aria-hidden /> },
			{ id: IDS.about, label: "About", icon: <Info className="size-4" aria-hidden /> },
		],
		[apiMissing, aiMissing],
	);

	const [activeId, setActiveId] = useActiveSection(sections, scroller, active && !!config);
	const jump = useCallback(
		(id: string) => {
			setActiveId(id);
			scrollToSection(id);
		},
		[setActiveId],
	);
	/** Checklist CTAs go straight to the control: the key field or the selected provider card. */
	const jumpToControl = useCallback(
		(section: "account" | "ai") => {
			const id = IDS[section];
			setActiveId(id);
			scrollToSection(id, false);
			const el = document.getElementById(id);
			const target =
				section === "account"
					? el?.querySelector<HTMLElement>("input")
					: el?.querySelector<HTMLElement>('[role="radio"][tabindex="0"]');
			if (target) target.focus({ preventScroll: true });
			else scrollToSection(id);
		},
		[setActiveId],
	);

	return (
		<div className="flex min-h-0 flex-1 flex-col">
			<PageHeader
				title="Settings"
				description="Your agent connection, AI analysis and preferences"
			/>
			<div ref={scroller} className="min-h-0 flex-1 overflow-y-auto">
				<div className="mx-auto grid max-w-[760px] grid-cols-1 gap-8 px-6 pb-16 pt-6 xl:max-w-[984px] xl:grid-cols-[192px_minmax(0,1fr)]">
					<aside className="hidden xl:block">
						<div className="sticky top-6">
							{config ? (
								<SectionNav sections={sections} activeId={activeId} onSelect={jump} />
							) : (
								<div className="space-y-2" aria-hidden>
									{sections.map((s) => (
										<div key={s.id} className="skeleton h-6 w-32" />
									))}
								</div>
							)}
						</div>
					</aside>

					<div className="min-w-0 space-y-5">
						{!config && configError ? (
							<div className="rounded-lg border border-line bg-surface-2">
								<ErrorState title="Could not load settings" message={configError} onRetry={() => void loadConfig()} />
							</div>
						) : !config ? (
							<div className="space-y-5" aria-busy="true" aria-label="Loading settings">
								<SectionSkeleton rows={1} />
								<SectionSkeleton rows={2} />
								<SectionSkeleton rows={2} />
								<SectionSkeleton rows={3} />
							</div>
						) : (
							<>
								<GetStarted config={config} onJump={jumpToControl} />
								<AccountSection id={IDS.account} config={config} active={active} />
								<AiSection id={IDS.ai} config={config} />
								<AutoTrainSection id={IDS.auto} config={config} />
								<AppearanceSection id={IDS.appearance} config={config} />
								<DataSection id={IDS.data} config={config} />
								<AboutSection id={IDS.about} />
							</>
						)}
					</div>
				</div>
			</div>
		</div>
	);
}
