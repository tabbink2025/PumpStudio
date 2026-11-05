import { useCallback, useEffect, useId, useRef, useState, type ReactNode } from "react";
import { Check, Copy, ExternalLink } from "lucide-react";
import { cn } from "../../lib/cn";
import { bun, errorMessage } from "../../lib/rpc";
import { toast } from "../../lib/store";
import { Button, IconButton } from "../ui/Button";
import { Dialog } from "../ui/overlays";

/* ---- Inline "Saved" feedback ------------------------------------------ */

/**
 * One transient "Saved" mark per section. mark(key) shows it next to the
 * control with that key for 2 seconds, replacing a toast for every toggle.
 */
export function useSavedFlash(): [string | null, (key: string) => void] {
	const [current, setCurrent] = useState<string | null>(null);
	const timer = useRef<ReturnType<typeof setTimeout>>();
	useEffect(() => () => clearTimeout(timer.current), []);
	const mark = useCallback((key: string) => {
		clearTimeout(timer.current);
		setCurrent(key);
		timer.current = setTimeout(() => setCurrent(null), 2000);
	}, []);
	return [current, mark];
}

export function SavedMark({ show, label = "Saved", className }: { show: boolean; label?: string; className?: string }) {
	return (
		<span
			role="status"
			aria-live="polite"
			className={cn(
				"inline-flex h-5 items-center gap-1 whitespace-nowrap text-sm font-medium text-pos transition-opacity duration-slow",
				show ? "opacity-100" : "opacity-0",
				className,
			)}
		>
			{show && (
				<>
					<Check className="size-3.5" aria-hidden />
					{label}
				</>
			)}
		</span>
	);
}

/* ---- Section card ------------------------------------------------------ */

export function SettingsSection({
	id,
	title,
	description,
	icon,
	action,
	children,
}: {
	id: string;
	title: string;
	description?: ReactNode;
	icon?: ReactNode;
	action?: ReactNode;
	children: ReactNode;
}) {
	const headingId = useId();
	return (
		<section
			id={id}
			data-settings-section
			aria-labelledby={headingId}
			className="scroll-mt-6 rounded-lg border border-line bg-surface-2 shadow-card"
		>
			<header className="flex items-start gap-3 border-b border-line px-5 py-4">
				{icon && (
					<div className="mt-0.5 flex size-7 shrink-0 items-center justify-center rounded-md border border-line-strong bg-surface-3 text-fg-2">
						{icon}
					</div>
				)}
				<div className="min-w-0 flex-1">
					<h2 id={headingId} className="text-md font-semibold text-fg">
						{title}
					</h2>
					{description && <p className="mt-0.5 text-base text-fg-3">{description}</p>}
				</div>
				{action && <div className="shrink-0">{action}</div>}
			</header>
			<div className="divide-y divide-line">{children}</div>
		</section>
	);
}

/** A row inside a section: label and helper on the left, control on the right. */
export function SettingRow({
	label,
	description,
	htmlFor,
	labelId,
	control,
	saved,
	children,
	stack,
}: {
	label: ReactNode;
	description?: ReactNode;
	htmlFor?: string;
	labelId?: string;
	control?: ReactNode;
	saved?: boolean;
	children?: ReactNode;
	/** Put the control under the label instead of beside it. */
	stack?: boolean;
}) {
	return (
		<div className="px-5 py-4">
			<div className={cn("flex gap-4", stack ? "flex-col gap-2.5" : "items-center justify-between")}>
				<div className="min-w-0 flex-1">
					<div className="flex items-center gap-2">
						{htmlFor ? (
							<label id={labelId} htmlFor={htmlFor} className="text-base font-medium text-fg">
								{label}
							</label>
						) : (
							<span id={labelId} className="text-base font-medium text-fg">
								{label}
							</span>
						)}
						{saved !== undefined && <SavedMark show={saved} />}
					</div>
					{description && <div className="mt-0.5 text-sm text-fg-3">{description}</div>}
				</div>
				{control && <div className={cn("min-w-0", !stack && "shrink-0")}>{control}</div>}
			</div>
			{children}
		</div>
	);
}

/* ---- Clipboard ------------------------------------------------------------ */

export async function copyText(text: string): Promise<boolean> {
	try {
		await navigator.clipboard.writeText(text);
		return true;
	} catch {
		try {
			const ta = document.createElement("textarea");
			ta.value = text;
			ta.style.position = "fixed";
			ta.style.opacity = "0";
			document.body.appendChild(ta);
			ta.select();
			const ok = document.execCommand("copy");
			ta.remove();
			return ok;
		} catch {
			return false;
		}
	}
}

export function CopyButton({ text, label, size = "sm" }: { text: string; label: string; size?: "sm" | "md" }) {
	const [copied, setCopied] = useState(false);
	const timer = useRef<ReturnType<typeof setTimeout>>();
	useEffect(() => () => clearTimeout(timer.current), []);
	async function copy() {
		const ok = await copyText(text);
		if (!ok) {
			toast({ kind: "error", title: "Could not copy", description: "Select the text and copy it manually." });
			return;
		}
		setCopied(true);
		clearTimeout(timer.current);
		timer.current = setTimeout(() => setCopied(false), 1500);
	}
	return (
		<IconButton label={copied ? "Copied" : label} size={size} onClick={() => void copy()}>
			{copied ? <Check className="size-3.5 text-pos" aria-hidden /> : <Copy className="size-3.5" aria-hidden />}
		</IconButton>
	);
}

/* ---- External links ---------------------------------------------------- */

export async function openLink(url: string) {
	try {
		await bun.openExternal(url);
	} catch (err) {
		toast({ kind: "error", title: "Could not open the link", description: errorMessage(err) });
	}
}

export function LinkButton({ href, children, className }: { href: string; children: ReactNode; className?: string }) {
	return (
		<button
			type="button"
			onClick={() => void openLink(href)}
			title={href}
			className={cn(
				"inline-flex items-center gap-1 rounded text-base font-medium text-info hover:underline hover:underline-offset-2",
				className,
			)}
		>
			{children}
			<ExternalLink className="size-3.5 shrink-0" aria-hidden />
		</button>
	);
}

/* ---- Confirm dialog ------------------------------------------------------ */

export function ConfirmDialog({
	open,
	onClose,
	onConfirm,
	title,
	description,
	confirmLabel,
	busy,
}: {
	open: boolean;
	onClose: () => void;
	onConfirm: () => void;
	title: string;
	description: ReactNode;
	confirmLabel: string;
	busy?: boolean;
}) {
	const titleId = useId();
	return (
		<Dialog
			open={open}
			onClose={onClose}
			title={title}
			labelledBy={titleId}
			width={420}
			footer={
				<>
					<Button variant="ghost" onClick={onClose} data-autofocus>
						Cancel
					</Button>
					<Button variant="danger" onClick={onConfirm} loading={busy}>
						{confirmLabel}
					</Button>
				</>
			}
		>
			<p className="text-base text-fg-2">{description}</p>
		</Dialog>
	);
}

/* ---- Switch row with inline saved mark ------------------------------------ */

export function SwitchRow({
	checked,
	onChange,
	label,
	description,
	saved,
	disabled,
}: {
	checked: boolean;
	onChange: (v: boolean) => void;
	label: string;
	description?: ReactNode;
	saved?: boolean;
	disabled?: boolean;
}) {
	const id = useId();
	const descId = useId();
	return (
		<div className="flex items-center justify-between gap-4 px-5 py-4">
			<div className="min-w-0">
				<div className="flex items-center gap-2">
					<label htmlFor={id} className="text-base font-medium text-fg">
						{label}
					</label>
					{saved !== undefined && <SavedMark show={saved} />}
				</div>
				{description && (
					<div id={descId} className="mt-0.5 text-sm text-fg-3">
						{description}
					</div>
				)}
			</div>
			<button
				id={id}
				type="button"
				role="switch"
				aria-checked={checked}
				aria-describedby={description ? descId : undefined}
				disabled={disabled}
				onClick={() => onChange(!checked)}
				className={cn(
					"relative inline-flex h-5 w-9 shrink-0 items-center rounded-full border transition-colors duration-fast disabled:opacity-50",
					checked ? "border-brand bg-brand" : "border-line-control bg-surface-3",
				)}
			>
				<span
					aria-hidden
					className={cn(
						"block size-3.5 rounded-full shadow-card transition-transform duration-fast",
						checked ? "translate-x-[18px] bg-brand-fg" : "translate-x-[3px] bg-fg-2",
					)}
				/>
			</button>
		</div>
	);
}
