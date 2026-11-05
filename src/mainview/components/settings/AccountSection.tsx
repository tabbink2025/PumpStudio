import { useEffect, useId, useRef, useState } from "react";
import { AlertTriangle, CheckCircle2, ChevronDown, CircleDashed, Plus, RotateCw, User, XCircle } from "lucide-react";
import type { KeyCheck, PublicConfig, RegisteredKey } from "../../../shared/types";
import { cn } from "../../lib/cn";
import { bun, errorMessage } from "../../lib/rpc";
import { useApp } from "../../lib/store";
import { Button } from "../ui/Button";
import { Badge, Banner, FieldLabel, FieldMessage, TextArea, TextInput } from "../ui/primitives";
import { SecretKeyField, type SubmitOutcome } from "./SecretKeyField";
import { CopyButton, LinkButton, SettingsSection, useSavedFlash } from "./shared";

const AGENTS_URL = "https://pump.studio/agents";
const NAME_MAX = 40;
const DESC_MAX = 200;

type CheckState = { kind: "idle" } | { kind: "checking" } | { kind: "done"; result: KeyCheck; at: number };

export function AccountSection({ id, config, active }: { id: string; config: PublicConfig; active: boolean }) {
	const saveConfig = useApp((s) => s.saveConfig);
	const [check, setCheck] = useState<CheckState>({ kind: "idle" });
	const [savedKey, markSaved] = useSavedFlash();
	const [created, setCreated] = useState<RegisteredKey | null>(null);
	const [createOpen, setCreateOpen] = useState(!config.apiKeySet);

	async function runCheck() {
		setCheck({ kind: "checking" });
		try {
			const result = await bun.checkApiKey();
			setCheck({ kind: "done", result, at: Date.now() });
		} catch (err) {
			setCheck({ kind: "done", result: { status: "unreachable", message: errorMessage(err) }, at: Date.now() });
		}
	}

	// Check the stored key once, the first time this screen is opened.
	const autoChecked = useRef(false);
	useEffect(() => {
		if (!active || autoChecked.current || !config.apiKeySet) return;
		autoChecked.current = true;
		void runCheck();
	}, [active, config.apiKeySet]);

	async function submitKey(value: string, force = false): Promise<SubmitOutcome> {
		let result: KeyCheck;
		if (force) {
			result = { status: "unreachable", message: "Saved without checking" };
		} else {
			try {
				result = await bun.checkApiKey(value);
			} catch (err) {
				result = { status: "unreachable", message: errorMessage(err) };
			}
		}
		if (result.status === "invalid") {
			return {
				ok: false,
				error: `Key rejected: ${result.message}.`,
				retry: { label: "Save anyway", run: () => submitKey(value, true) },
			};
		}
		const next = await saveConfig({ apiKey: value }, { silent: true });
		if (!next) return { ok: false, error: "Could not save the key. Try again." };
		setCheck(force ? { kind: "idle" } : { kind: "done", result, at: Date.now() });
		markSaved("apiKey");
		return { ok: true };
	}

	async function removeKey() {
		const next = await saveConfig({ apiKey: null }, { silent: true });
		if (next) {
			setCheck({ kind: "idle" });
			setCreated(null);
			setCreateOpen(true);
			useApp.getState().toast({ kind: "info", title: "pump.studio key removed", duration: 3000 });
		}
		return !!next;
	}

	return (
		<SettingsSection
			id={id}
			title="Account"
			icon={<User className="size-4" aria-hidden />}
			description="Your pump.studio agent key. It signs every analysis you submit, so XP lands on your agent."
			action={
				config.apiKeySet ? (
					<Badge tone="pos" dot>
						Key saved
					</Badge>
				) : (
					<Badge tone="warn" dot>
						Not connected
					</Badge>
				)
			}
		>
			<div className="px-5 py-4">
				<SecretKeyField
					label="pump.studio API key"
					isSet={config.apiKeySet}
					hint={config.apiKeyHint}
					placeholder="ps_…"
					helper="We check the key with pump.studio before saving it."
					submitLabel="Save and check"
					saved={savedKey === "apiKey"}
					onSubmit={(v) => submitKey(v)}
					onRemove={removeKey}
					removeTitle="Remove pump.studio key?"
					removeDescription="You can still browse tokens, but submitting analyses and auto train stop working until you add a key again."
					status={<ConnectionStatus state={check} onCheck={() => void runCheck()} />}
					extra={<LinkButton href={AGENTS_URL}>Get a key on pump.studio</LinkButton>}
				/>
			</div>

			{created && (
				<div className="px-5 py-4">
					<CreatedKey created={created} onDone={() => setCreated(null)} />
				</div>
			)}

			{!created && (
				<CreateKeyPanel
					open={createOpen}
					onToggle={() => setCreateOpen((o) => !o)}
					replacing={config.apiKeySet}
					onCreated={async (k) => {
						const next = await saveConfig({ apiKey: k.key }, { silent: true });
						setCreated(k);
						setCreateOpen(false);
						if (next) {
							markSaved("apiKey");
							void runCheck();
						}
					}}
				/>
			)}
		</SettingsSection>
	);
}

/* ---- Connection status -------------------------------------------------- */

function ConnectionStatus({ state, onCheck }: { state: CheckState; onCheck: () => void }) {
	let icon = <CircleDashed className="size-3.5 text-fg-3" aria-hidden />;
	let text = <span className="text-fg-3">Connection not checked yet</span>;
	if (state.kind === "checking") {
		icon = <RotateCw className="size-3.5 animate-spin text-fg-3" aria-hidden />;
		text = <span className="text-fg-2">Checking with pump.studio…</span>;
	} else if (state.kind === "done") {
		const r = state.result;
		if (r.status === "valid") {
			icon = <CheckCircle2 className="size-3.5 text-pos" aria-hidden />;
			text = (
				<span className="text-pos">
					{r.agentName ? (
						<>
							Connected as <span className="font-semibold">{r.agentName}</span>
						</>
					) : (
						"Connected"
					)}
				</span>
			);
		} else if (r.status === "invalid") {
			icon = <XCircle className="size-3.5 text-neg" aria-hidden />;
			text = (
				<span className="min-w-0 truncate text-neg" title={r.message}>
					Key rejected <span className="text-fg-3">· {r.message}</span>
				</span>
			);
		} else if (r.status === "unreachable") {
			icon = <AlertTriangle className="size-3.5 text-warn" aria-hidden />;
			text = (
				<span className="min-w-0 truncate text-warn" title={r.message}>
					pump.studio unreachable <span className="text-fg-3">· {r.message}</span>
				</span>
			);
		} else {
			icon = <AlertTriangle className="size-3.5 text-warn" aria-hidden />;
			text = <span className="text-warn">No key to check</span>;
		}
	}
	return (
		<div className="flex min-h-7 items-center gap-2 text-sm">
			<span className="flex min-w-0 flex-1 items-center gap-1.5" role="status" aria-live="polite">
				{icon}
				{text}
			</span>
			<Button
				size="sm"
				variant="ghost"
				onClick={onCheck}
				disabled={state.kind === "checking"}
				icon={<RotateCw className="size-3.5" aria-hidden />}
			>
				Check connection
			</Button>
		</div>
	);
}

/* ---- Create a new agent key --------------------------------------------- */

function CreateKeyPanel({
	open,
	onToggle,
	onCreated,
	replacing,
}: {
	open: boolean;
	onToggle: () => void;
	onCreated: (k: RegisteredKey) => Promise<void>;
	replacing: boolean;
}) {
	const nameId = useId();
	const descId = useId();
	const panelId = useId();
	const [name, setName] = useState("");
	const [desc, setDesc] = useState("");
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<string | null>(null);
	const [touched, setTouched] = useState(false);
	const nameError = touched && !name.trim() ? "Give your agent a name" : null;

	async function create() {
		setTouched(true);
		if (!name.trim()) return;
		setBusy(true);
		setError(null);
		try {
			const k = await bun.registerApiKey(name.trim().slice(0, NAME_MAX), desc.trim() || undefined);
			await onCreated(k);
			setName("");
			setDesc("");
			setTouched(false);
		} catch (err) {
			setError(errorMessage(err));
		} finally {
			setBusy(false);
		}
	}

	return (
		<div>
			<button
				type="button"
				aria-expanded={open}
				aria-controls={panelId}
				onClick={onToggle}
				className="focus-inset flex w-full items-center gap-3 px-5 py-3 text-left hover:bg-surface-3/50"
			>
				<Plus className="size-4 shrink-0 text-fg-3" aria-hidden />
				<span className="min-w-0 flex-1">
					<span className="block text-base font-medium text-fg">Create a new agent key</span>
					<span className="block text-sm text-fg-3">
						{replacing ? "Registers a new agent and replaces the key above." : "No key yet? Register an agent right here."}
					</span>
				</span>
				<ChevronDown className={cn("size-4 shrink-0 text-fg-3 transition-transform duration-fast", open && "rotate-180")} aria-hidden />
			</button>
			{open && (
				<form
					id={panelId}
					className="grid gap-3 px-5 pb-4 pt-1"
					onSubmit={(e) => {
						e.preventDefault();
						void create();
					}}
				>
					<div className="grid grid-cols-1 gap-3 lg:grid-cols-[minmax(0,1fr)_minmax(0,1.4fr)]">
						<div>
							<FieldLabel htmlFor={nameId} hint={<span className="num">{name.length}/{NAME_MAX}</span>}>
								Agent name
							</FieldLabel>
							<TextInput
								id={nameId}
								value={name}
								maxLength={NAME_MAX}
								required
								placeholder="e.g. my-sol-analyst"
								invalid={!!nameError}
								aria-describedby={nameError ? `${nameId}-err` : undefined}
								onBlur={() => name && setTouched(true)}
								onChange={(e) => setName(e.target.value)}
							/>
							{nameError && (
								<FieldMessage error id={`${nameId}-err`}>
									{nameError}
								</FieldMessage>
							)}
						</div>
						<div>
							<FieldLabel htmlFor={descId} hint="Optional">
								Description
							</FieldLabel>
							<TextArea
								id={descId}
								value={desc}
								rows={1}
								maxLength={DESC_MAX}
								placeholder="What this agent does"
								className="h-8 py-1.5"
								onChange={(e) => setDesc(e.target.value)}
								onKeyDown={(e) => {
									if (e.key === "Enter" && !e.shiftKey) {
										e.preventDefault();
										void create();
									}
								}}
							/>
						</div>
					</div>
					{error && (
						<Banner tone="neg" title="Could not create the key">
							{error}
						</Banner>
					)}
					<div className="flex items-center gap-3">
						<Button type="submit" variant={replacing ? "secondary" : "primary"} loading={busy}>
							Create key
						</Button>
						<span className="text-sm text-fg-3">The key is saved here and shown once so you can copy it.</span>
					</div>
				</form>
			)}
		</div>
	);
}

function CreatedKey({ created, onDone }: { created: RegisteredKey; onDone: () => void }) {
	return (
		<div className="rounded-lg border border-pos/25 bg-pos/5 p-3">
			<div className="flex items-center gap-2">
				<CheckCircle2 className="size-4 text-pos" aria-hidden />
				<span className="text-base font-medium text-fg">Agent key created and saved</span>
				{created.type && <Badge>{created.type}</Badge>}
				{created.rateLimit != null && (
					<span className="num text-sm text-fg-3">Rate limit {created.rateLimit}</span>
				)}
			</div>
			<div className="mt-2.5 flex items-center gap-1 rounded-md border border-line-strong bg-surface-1 py-1 pl-2.5 pr-1">
				<code className="selectable min-w-0 flex-1 break-all font-mono text-sm text-fg">{created.key}</code>
				<CopyButton text={created.key} label="Copy key" />
			</div>
			<div className="mt-2.5 flex items-start justify-between gap-3">
				<p className="flex items-start gap-1.5 text-sm text-warn">
					<AlertTriangle className="mt-0.5 size-3.5 shrink-0" aria-hidden />
					Copy it now and keep it safe. You can always paste it again here.
				</p>
				<Button size="sm" onClick={onDone}>
					Done
				</Button>
			</div>
		</div>
	);
}
