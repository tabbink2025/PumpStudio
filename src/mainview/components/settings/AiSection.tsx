import { useEffect, useId, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { Check, Gauge, Lock, Sparkles } from "lucide-react";
import type { Orchestrator, PublicConfig } from "../../../shared/types";
import { cn } from "../../lib/cn";
import { useApp } from "../../lib/store";
import { Badge, FieldLabel, FieldMessage, TextInput } from "../ui/primitives";
import { SecretKeyField } from "./SecretKeyField";
import { CopyButton, SavedMark, SettingsSection, useSavedFlash } from "./shared";

export const AI_CHOSEN_KEY = "pumpstudio.settings.aiChosen";

interface Provider {
	id: Orchestrator;
	name: string;
	vendor: string;
	blurb: string;
	icon: ReactNode;
	badge?: ReactNode;
}

const PROVIDERS: Provider[] = [
	{
		id: "manual",
		name: "Data rules",
		vendor: "Built in, no key",
		blurb: "Free and instant. Uses the official Pump Studio heuristics.",
		icon: <Gauge className="size-4" aria-hidden />,
	},
	{
		id: "claude",
		name: "Claude",
		vendor: "Anthropic",
		blurb: "Reads the full snapshot and writes a reasoned summary.",
		icon: <Sparkles className="size-4" aria-hidden />,
		badge: <Badge tone="brand">Recommended</Badge>,
	},
	{
		id: "openai",
		name: "OpenAI",
		vendor: "GPT models",
		blurb: "Same prompt and checks, billed to your OpenAI key.",
		icon: <Sparkles className="size-4" aria-hidden />,
	},
];

const PROVIDER_META = {
	claude: {
		keyField: "claudeApiKey",
		modelField: "claudeModel",
		defaultModel: "claude-opus-5-5",
		placeholder: "sk-ant-…",
		keyLabel: "Anthropic API key",
		consoleName: "Anthropic",
	},
	openai: {
		keyField: "openaiApiKey",
		modelField: "openaiModel",
		defaultModel: "gpt-5.5",
		placeholder: "sk-…",
		keyLabel: "OpenAI API key",
		consoleName: "OpenAI",
	},
} as const;

export const AI_CHOSEN_EVENT = "pumpstudio:ai-chosen";

function markChosen() {
	try {
		localStorage.setItem(AI_CHOSEN_KEY, "1");
	} catch {}
	window.dispatchEvent(new Event(AI_CHOSEN_EVENT));
}

export function AiSection({ id, config }: { id: string; config: PublicConfig }) {
	const saveConfig = useApp((s) => s.saveConfig);
	const [saved, markSaved] = useSavedFlash();
	const refs = useRef<Array<HTMLButtonElement | null>>([]);
	const groupLabel = useId();

	const selected = config.orchestrator;
	const keySet = (p: Orchestrator) => (p === "claude" ? config.claudeKeySet : p === "openai" ? config.openaiKeySet : true);

	async function choose(p: Orchestrator) {
		markChosen();
		if (p === selected) return;
		// Optimistic so the radio moves instantly.
		const prev = useApp.getState().config;
		if (prev) useApp.setState({ config: { ...prev, orchestrator: p } });
		const next = await saveConfig({ orchestrator: p }, { silent: true });
		if (next) markSaved("orchestrator");
		else if (prev) useApp.setState({ config: { ...useApp.getState().config!, orchestrator: prev.orchestrator } });
	}

	function onKey(e: KeyboardEvent) {
		const idx = PROVIDERS.findIndex((p) => p.id === selected);
		let next = -1;
		if (e.key === "ArrowRight" || e.key === "ArrowDown") next = (idx + 1) % PROVIDERS.length;
		if (e.key === "ArrowLeft" || e.key === "ArrowUp") next = (idx - 1 + PROVIDERS.length) % PROVIDERS.length;
		if (e.key === "Home") next = 0;
		if (e.key === "End") next = PROVIDERS.length - 1;
		if (next >= 0) {
			e.preventDefault();
			void choose(PROVIDERS[next]!.id);
			refs.current[next]?.focus();
		}
	}

	return (
		<SettingsSection
			id={id}
			title="AI analysis"
			icon={<Sparkles className="size-4" aria-hidden />}
			description="What fills in the analysis form in Analyze and auto train. You review before you submit."
		>
			<div className="px-5 py-4">
				<div className="mb-2 flex items-center gap-2">
					<span id={groupLabel} className="text-sm font-medium text-fg-2">
						Fill analyses with
					</span>
					<SavedMark show={saved === "orchestrator"} />
				</div>
				<div role="radiogroup" aria-labelledby={groupLabel} onKeyDown={onKey} className="grid grid-cols-3 gap-2">
					{PROVIDERS.map((p, i) => {
						const isSel = p.id === selected;
						const needsKey = p.id !== "manual" && !keySet(p.id);
						return (
							<button
								key={p.id}
								ref={(el) => {
									refs.current[i] = el;
								}}
								type="button"
								role="radio"
								aria-checked={isSel}
								tabIndex={isSel ? 0 : -1}
								onClick={() => void choose(p.id)}
								className={cn(
									"relative flex min-w-0 flex-col items-start rounded-lg border p-3 text-left transition-colors duration-fast",
									isSel
										? "border-brand/50 bg-brand/5"
										: "border-line-strong bg-surface-1 hover:border-line-control hover:bg-surface-3",
								)}
							>
								<div className="flex w-full items-center gap-2">
									<span className={cn("shrink-0", isSel ? "text-brand" : "text-fg-3")}>{p.icon}</span>
									<span className="truncate text-base font-semibold text-fg">{p.name}</span>
									<span
										aria-hidden
										className={cn(
											"ml-auto flex size-4 shrink-0 items-center justify-center rounded-full border",
											isSel ? "border-brand bg-brand text-brand-fg" : "border-line-control",
										)}
									>
										{isSel && <Check className="size-3" strokeWidth={3} />}
									</span>
								</div>
								<div className="mt-0.5 text-sm text-fg-3">{p.vendor}</div>
								<p className="mt-2 text-sm text-fg-2">{p.blurb}</p>
								<div className="mt-2.5 flex min-h-5 flex-wrap items-center gap-1.5">
									{p.id === "manual" ? (
										<Badge tone="neutral">Free</Badge>
									) : needsKey ? (
										<Badge tone="warn">Needs key</Badge>
									) : (
										<Badge tone="pos" icon={<Check className="size-3" aria-hidden />}>
											Key saved
										</Badge>
									)}
									{p.badge}
								</div>
							</button>
						);
					})}
				</div>
			</div>

			{selected !== "manual" && <ProviderSettings key={selected} provider={selected} config={config} />}

			<div className="flex items-start gap-2.5 px-5 py-3.5">
				<Lock className="mt-0.5 size-3.5 shrink-0 text-fg-3" aria-hidden />
				<p className="min-w-0 flex-1 text-sm text-fg-3">
					Keys are stored only on this computer in{" "}
					<code className="selectable break-all font-mono text-fg-2">{config.configPath}</code>, readable only by your
					user account. An AI key is sent only to its own provider.
				</p>
				<CopyButton text={config.configPath} label="Copy config file path" />
			</div>
		</SettingsSection>
	);
}

function ProviderSettings({ provider, config }: { provider: "claude" | "openai"; config: PublicConfig }) {
	const meta = PROVIDER_META[provider];
	const saveConfig = useApp((s) => s.saveConfig);
	const [saved, markSaved] = useSavedFlash();
	const modelId = useId();
	const stored = config[meta.modelField];
	const [model, setModel] = useState(stored);
	useEffect(() => setModel(stored), [stored]);

	const isSet = provider === "claude" ? config.claudeKeySet : config.openaiKeySet;
	const hint = provider === "claude" ? config.claudeKeyHint : config.openaiKeyHint;

	async function saveModel(value = model) {
		const v = value.trim() || meta.defaultModel;
		setModel(v);
		if (v === stored) return;
		const next = await saveConfig({ [meta.modelField]: v }, { silent: true });
		if (next) markSaved("model");
		else setModel(stored);
	}

	return (
		<div className="grid gap-4 px-5 py-4">
			<SecretKeyField
				label={meta.keyLabel}
				isSet={isSet}
				hint={hint}
				placeholder={meta.placeholder}
				helper={
					isSet
						? undefined
						: `Paste a key from your ${meta.consoleName} account. Until then, analyses use the data rules.`
				}
				submitLabel="Save key"
				saved={saved === "key"}
				onSubmit={async (v) => {
					const next = await saveConfig({ [meta.keyField]: v }, { silent: true });
					markChosen();
					if (!next) return { ok: false, error: "Could not save the key. Try again." };
					markSaved("key");
					return { ok: true };
				}}
				onRemove={async () => {
					const next = await saveConfig({ [meta.keyField]: null }, { silent: true });
					if (next) markSaved("key");
					return !!next;
				}}
				removeTitle={`Remove ${meta.keyLabel}?`}
				removeDescription="AI fill will fall back to the data rules until you add a key again."
			/>

			<div>
				<FieldLabel
					htmlFor={modelId}
					hint={
						model.trim() !== meta.defaultModel ? (
							<button
								type="button"
								className="rounded text-sm text-fg-3 underline decoration-dotted underline-offset-2 hover:text-fg"
								// Keep focus in the field so its blur does not save the typed value first.
								onMouseDown={(e) => e.preventDefault()}
								onClick={() => void saveModel(meta.defaultModel)}
							>
								Use default {meta.defaultModel}
							</button>
						) : (
							<SavedMark show={saved === "model"} />
						)
					}
				>
					Model
				</FieldLabel>
				<div className="flex items-center gap-2">
					<TextInput
						id={modelId}
						mono
						value={model}
						spellCheck={false}
						autoComplete="off"
						placeholder={meta.defaultModel}
						className="max-w-[320px]"
						aria-describedby={`${modelId}-help`}
						onChange={(e) => setModel(e.target.value)}
						onBlur={() => void saveModel()}
						onKeyDown={(e) => {
							if (e.key === "Enter") {
								e.preventDefault();
								void saveModel();
							}
							if (e.key === "Escape") {
								e.preventDefault();
								setModel(stored);
							}
						}}
					/>
					{model.trim() !== meta.defaultModel && <SavedMark show={saved === "model"} />}
				</div>
				<FieldMessage id={`${modelId}-help`}>Model id from your provider. Saved when you leave the field.</FieldMessage>
			</div>
		</div>
	);
}
