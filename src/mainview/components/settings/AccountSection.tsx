import { useEffect, useId, useRef, useState } from "react";
import { AlertTriangle, CheckCircle2, ChevronDown, CircleDashed, Plus, RotateCw, User, XCircle } from "lucide-react";
import type { KeyCheck, PublicConfig, RegisteredKey } from "../../../shared/types";
import { cn } from "../../lib/cn";
import { bun, errorMessage } from "../../lib/rpc";
import { useApp } from "../../lib/store";
import { Button } from "../ui/Button";
import { Badge, Banner, FieldLabel, FieldMessage, TextInput } from "../ui/primitives";
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
	const [created, setCreated] = useState<{ key: RegisteredKey; saved: boolean } | null>(null);
	const [createOpen, setCreateOpen] = useState(!config.apiKeySet);
	// Collapse the create panel once a key is in place, open it again when removed.
	useEffect(() => setCreateOpen(!config.apiKeySet), [config.apiKeySet]);

	// Every status change bumps the sequence, so a slow check that finishes
	// after the key was replaced or removed cannot overwrite the newer status.
	const checkSeq = useRef(0);
	function setCheckNow(next: CheckState) {
		checkSeq.current += 1;
		setCheck(next);
	}

	async function runCheck() {
		setCheckNow({ kind: "checking" });
		const seq = checkSeq.current;
		let result: KeyCheck;
		try {
			result = await bun.checkApiKey();
		} catch (err) {
			result = { status: "unreachable", message: errorMessage(err) };
		}
		if (seq === checkSeq.current) setCheck({ kind: "done", result, at: Date.now() });
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
				error: `Key rejected: ${result.message}`,
				retry: { label: "Save anyway", run: () => submitKey(value, true) },
			};
		}
		const next = await saveConfig({ apiKey: value }, { silent: true });
		if (!next) return { ok: false, error: "Could not save the key. Try again." };
		setCheckNow(force ? { kind: "idle" } : { kind: "done", result, at: Date.now() });
		markSaved("apiKey");
		return { ok: true };
	}

	async function removeKey() {
		const next = await saveConfig({ apiKey: null }, { silent: true });
		if (next) {
			setCheckNow({ kind: "idle" });
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
			description="Your agent key signs every analysis you submit, so XP lands on your agent."
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
					<CreatedKey created={created.key} saved={created.saved} onDone={() => setCreated(null)} />
				</div>
			)}

			{!created && (
				<CreateKeyPanel
					open={createOpen}
					onToggle={() => setCreateOpen((o) => !o)}
					replacing={config.apiKeySet}
					onCreated={async (k) => {
						const next = await saveConfig({ apiKey: k.key }, { silent: true });
						// Show the key either way: if saving failed it is the only copy.
						setCreated({ key: k, saved: !!next });
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
							<TextInput
								id={descId}
								value={desc}
								maxLength={DESC_MAX}
								placeholder="What this agent does"
								onChange={(e) => setDesc(e.target.value)}
							/>
						</div>
					</div>
					{error && (
						<Banner tone="neg" title="Could not create the key">
							{error}
						</Banner>
					)}
					<div className="flex items-center gap-3">
						<Button type="submit" loading={busy} icon={<Plus className="size-3.5" aria-hidden />}>
							Create key
						</Button>
						<span className="text-sm text-fg-3">The key is saved here and shown once so you can copy it.</span>
					</div>
				</form>
			)}
		</div>
	);
}

function CreatedKey({ created, saved, onDone }: { created: RegisteredKey; saved: boolean; onDone: () => void }) {
	return (
		<div className={cn("rounded-lg border p-3", saved ? "border-pos/25 bg-pos/5" : "border-warn/25 bg-warn/5")}>
			<div className="flex flex-wrap items-center gap-2">
				{saved ? (
					<CheckCircle2 className="size-4 shrink-0 text-pos" aria-hidden />
				) : (
					<AlertTriangle className="size-4 shrink-0 text-warn" aria-hidden />
				)}
				<span className="text-base font-medium text-fg">
					{saved ? "Agent key created and saved" : "Agent key created but not saved"}
				</span>
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
					{saved
						? "Copy it now and keep it safe. You can always paste it again here."
						: "Copy it now and paste it in the key field above. It is not shown again."}
				</p>
				<Button size="sm" onClick={onDone}>
					Done
				</Button>
			</div>
		</div>
	);
}
