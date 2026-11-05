import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { ClipboardPaste, Eye, EyeOff, KeyRound } from "lucide-react";
import { errorMessage } from "../../lib/rpc";
import { toast } from "../../lib/store";
import { Button, IconButton } from "../ui/Button";
import { FieldMessage } from "../ui/primitives";
import { ConfirmDialog, SavedMark } from "./shared";

export type SubmitOutcome =
	| { ok: true }
	| { ok: false; error: ReactNode; retry?: { label: string; run: () => Promise<SubmitOutcome> } };

/**
 * A secret stored by the Bun process. The view never sees the key itself:
 * when set it shows the masked hint with Replace and Remove, otherwise (or
 * while replacing) a password field with show/hide and paste.
 */
export function SecretKeyField({
	label,
	isSet,
	hint,
	placeholder,
	helper,
	submitLabel,
	onSubmit,
	onRemove,
	removeTitle,
	removeDescription,
	status,
	extra,
	saved,
	autoFocusOnReplace = true,
}: {
	label: string;
	isSet: boolean;
	hint: string;
	placeholder: string;
	helper?: ReactNode;
	submitLabel: string;
	onSubmit: (value: string) => Promise<SubmitOutcome>;
	onRemove: () => Promise<boolean>;
	removeTitle: string;
	removeDescription: ReactNode;
	/** Shown under the masked key, e.g. the connection check. */
	status?: ReactNode;
	/** Shown under the input while editing, e.g. a "Get a key" link. */
	extra?: ReactNode;
	saved?: boolean;
	autoFocusOnReplace?: boolean;
}) {
	const id = useId();
	const msgId = useId();
	const inputRef = useRef<HTMLInputElement>(null);
	const replaceRef = useRef<HTMLButtonElement>(null);
	const [replacing, setReplacing] = useState(false);
	const [value, setValue] = useState("");
	const [reveal, setReveal] = useState(false);
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<ReactNode>(null);
	const [retry, setRetry] = useState<{ label: string; run: () => Promise<SubmitOutcome> } | null>(null);
	const [confirmRemove, setConfirmRemove] = useState(false);
	const [removing, setRemoving] = useState(false);

	const editing = !isSet || replacing;

	// Leave replace mode when the stored key changes elsewhere (removed, or a
	// newly created agent key was saved while this field was open).
	useEffect(() => {
		setReplacing(false);
		setValue("");
		setError(null);
		setRetry(null);
	}, [isSet, hint]);

	/** The input unmounts on cancel/save, so hand focus to Replace key. */
	function focusReplace() {
		setTimeout(() => replaceRef.current?.focus(), 0);
	}

	function startReplace() {
		setReplacing(true);
		setValue("");
		setError(null);
		if (autoFocusOnReplace) setTimeout(() => inputRef.current?.focus(), 0);
	}

	function cancel() {
		setReplacing(false);
		setValue("");
		setError(null);
		setRetry(null);
		setReveal(false);
		focusReplace();
	}

	async function submit() {
		const v = value.trim();
		if (!v) {
			setError("Paste a key first");
			inputRef.current?.focus();
			return;
		}
		await run(() => onSubmit(v));
	}

	async function run(fn: () => Promise<SubmitOutcome>) {
		setBusy(true);
		setError(null);
		setRetry(null);
		let r: SubmitOutcome;
		try {
			r = await fn();
		} catch (err) {
			r = { ok: false, error: errorMessage(err) };
		} finally {
			setBusy(false);
		}
		if (r.ok) {
			setValue("");
			setReveal(false);
			setReplacing(false);
			focusReplace();
		} else {
			setError(r.error);
			setRetry(r.retry ?? null);
		}
	}

	async function paste() {
		try {
			const text = (await navigator.clipboard.readText()).trim();
			if (!text) {
				toast({ kind: "info", title: "Clipboard is empty", duration: 2500 });
				return;
			}
			setValue(text);
			setError(null);
			inputRef.current?.focus();
		} catch {
			toast({ kind: "error", title: "Could not read the clipboard", description: "Paste with the keyboard instead." });
			inputRef.current?.focus();
		}
	}

	async function remove() {
		setRemoving(true);
		let ok = false;
		try {
			ok = await onRemove();
		} finally {
			setRemoving(false);
		}
		if (ok) {
			setConfirmRemove(false);
			// The Remove button is gone; continue in the empty key field.
			setTimeout(() => inputRef.current?.focus(), 0);
		}
	}

	return (
		<div>
			<div className="mb-1.5 flex items-center gap-2">
				{editing ? (
					<label htmlFor={id} id={`${id}-label`} className="text-sm font-medium text-fg-2">
						{label}
					</label>
				) : (
					<span id={`${id}-label`} className="text-sm font-medium text-fg-2">
						{label}
					</span>
				)}
				{saved !== undefined && <SavedMark show={saved} />}
			</div>

			{!editing ? (
				<>
					<div className="flex flex-wrap items-center gap-2">
						<div
							role="group"
							aria-labelledby={`${id}-label`}
							className="flex h-8 min-w-0 flex-1 items-center gap-2 rounded-md border border-line bg-surface-1 px-2.5"
						>
							<KeyRound className="size-3.5 shrink-0 text-fg-3" aria-hidden />
							<span className="selectable truncate font-mono text-sm text-fg" title="Masked key, the full key stays on this computer">
								{hint || "Key saved"}
							</span>
						</div>
						<Button ref={replaceRef} size="md" onClick={startReplace}>
							Replace key
						</Button>
						<Button size="md" variant="ghost" className="text-neg hover:text-neg" onClick={() => setConfirmRemove(true)}>
							Remove
						</Button>
					</div>
					{status && <div className="mt-2">{status}</div>}
				</>
			) : (
				<>
					<form
						className="flex flex-wrap items-center gap-2"
						onSubmit={(e) => {
							e.preventDefault();
							void submit();
						}}
					>
						<div
							className="field flex h-8 min-w-[220px] flex-1 items-center pl-2.5 pr-0.5"
							aria-invalid={error ? true : undefined}
						>
							<input
								ref={inputRef}
								id={id}
								type={reveal ? "text" : "password"}
								value={value}
								onChange={(e) => {
									setValue(e.target.value);
									if (error) {
										setError(null);
										setRetry(null);
									}
								}}
								onKeyDown={(e) => {
									if (e.key === "Escape" && isSet) {
										e.preventDefault();
										cancel();
									}
								}}
								placeholder={placeholder}
								autoComplete="off"
								spellCheck={false}
								aria-invalid={error ? true : undefined}
								aria-describedby={msgId}
								className="h-full min-w-0 flex-1 bg-transparent font-mono text-sm text-fg outline-none placeholder:text-fg-3"
							/>
							<IconButton
								label={reveal ? "Hide key" : "Show key"}
								size="sm"
								aria-pressed={reveal}
								onClick={() => setReveal((r) => !r)}
							>
								{reveal ? <EyeOff className="size-3.5" aria-hidden /> : <Eye className="size-3.5" aria-hidden />}
							</IconButton>
						</div>
						<Button icon={<ClipboardPaste className="size-3.5" aria-hidden />} onClick={() => void paste()}>
							Paste
						</Button>
						<Button type="submit" variant="primary" loading={busy}>
							{submitLabel}
						</Button>
						{isSet && (
							<Button variant="ghost" onClick={cancel} disabled={busy}>
								Cancel
							</Button>
						)}
					</form>
					<div id={msgId}>
						{error ? (
							<FieldMessage error>
								{error}
								{retry && (
									<>
										{" "}
										<button
											type="button"
											disabled={busy}
											onClick={() => void run(retry.run)}
											className="rounded font-medium text-fg-2 underline underline-offset-2 hover:text-fg"
										>
											{retry.label}
										</button>
									</>
								)}
							</FieldMessage>
						) : (
							helper && <FieldMessage>{helper}</FieldMessage>
						)}
					</div>
					{extra && <div className="mt-2.5">{extra}</div>}
				</>
			)}

			<ConfirmDialog
				open={confirmRemove}
				onClose={() => setConfirmRemove(false)}
				onConfirm={() => void remove()}
				busy={removing}
				title={removeTitle}
				description={removeDescription}
				confirmLabel="Remove key"
			/>
		</div>
	);
}

