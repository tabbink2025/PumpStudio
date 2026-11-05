import { useEffect, useRef, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { CheckCircle2, Info, Sparkles, X, XCircle } from "lucide-react";
import { cn } from "../../lib/cn";
import { useApp, type Toast } from "../../lib/store";
import { IconButton } from "./Button";

/* ---- Toasts ---------------------------------------------------------- */

function ToastItem({ t }: { t: Toast }) {
	const dismiss = useApp((s) => s.dismissToast);
	const timer = useRef<ReturnType<typeof setTimeout>>();
	const start = () => {
		if (t.duration > 0) timer.current = setTimeout(() => dismiss(t.id), t.duration);
	};
	const pause = () => clearTimeout(timer.current);
	useEffect(() => {
		start();
		return pause;
		// eslint-disable-next-line react-hooks/exhaustive-deps
	}, [t.id]);

	const Icon = t.kind === "error" ? XCircle : t.kind === "success" ? CheckCircle2 : t.kind === "xp" ? Sparkles : Info;
	const color = { error: "text-neg", success: "text-pos", xp: "text-brand", info: "text-info" }[t.kind];

	return (
		<div
			role={t.kind === "error" ? "alert" : "status"}
			onMouseEnter={pause}
			onMouseLeave={start}
			className="pointer-events-auto flex w-[360px] animate-toast-in items-start gap-3 rounded-lg border border-line-strong bg-surface-2 p-3 shadow-pop"
		>
			<Icon className={cn("mt-0.5 size-4 shrink-0", color)} aria-hidden />
			<div className="min-w-0 flex-1">
				<div className="text-base font-medium text-fg">{t.title}</div>
				{t.description && <div className="mt-0.5 break-words text-sm text-fg-2">{t.description}</div>}
				{t.action && (
					<button
						type="button"
						onClick={() => {
							t.action!.run();
							dismiss(t.id);
						}}
						className="mt-2 rounded text-sm font-medium text-info hover:underline"
					>
						{t.action.label}
					</button>
				)}
			</div>
			<IconButton label="Dismiss" size="sm" onClick={() => dismiss(t.id)} className="-mr-1 -mt-1">
				<X className="size-3.5" />
			</IconButton>
		</div>
	);
}

export function Toaster() {
	const toasts = useApp((s) => s.toasts);
	return createPortal(
		<div className="pointer-events-none fixed bottom-20 right-5 z-[60] flex flex-col-reverse gap-2" aria-live="polite">
			{[...toasts].reverse().map((t) => (
				<ToastItem key={t.id} t={t} />
			))}
		</div>,
		document.body,
	);
}

/* ---- Dialog ---------------------------------------------------------- */

export function Dialog({
	open,
	onClose,
	title,
	description,
	children,
	footer,
	width = 480,
	labelledBy,
}: {
	open: boolean;
	onClose: () => void;
	title?: ReactNode;
	description?: ReactNode;
	children?: ReactNode;
	footer?: ReactNode;
	width?: number;
	labelledBy?: string;
}) {
	const panel = useRef<HTMLDivElement>(null);
	const restore = useRef<Element | null>(null);
	// Keep the latest onClose without re-running the focus effect (callers often pass inline arrows).
	const closeRef = useRef(onClose);
	closeRef.current = onClose;

	useEffect(() => {
		if (!open) return;
		restore.current = document.activeElement;
		const t = setTimeout(() => {
			const el =
				panel.current?.querySelector<HTMLElement>("[data-autofocus]") ??
				panel.current?.querySelector<HTMLElement>("input, textarea, button:not([data-close])");
			(el ?? panel.current)?.focus();
		}, 0);
		const onKey = (e: KeyboardEvent) => {
			if (e.key === "Escape") {
				e.stopPropagation();
				closeRef.current();
			}
			if (e.key === "Tab" && panel.current) {
				const f = panel.current.querySelectorAll<HTMLElement>(
					'button:not([disabled]), [href], input:not([disabled]), textarea, select, [tabindex]:not([tabindex="-1"])',
				);
				if (f.length === 0) return;
				const first = f[0]!;
				const last = f[f.length - 1]!;
				if (e.shiftKey && document.activeElement === first) {
					e.preventDefault();
					last.focus();
				} else if (!e.shiftKey && document.activeElement === last) {
					e.preventDefault();
					first.focus();
				}
			}
		};
		window.addEventListener("keydown", onKey, true);
		return () => {
			clearTimeout(t);
			window.removeEventListener("keydown", onKey, true);
			(restore.current as HTMLElement | null)?.focus?.();
		};
	}, [open]);

	if (!open) return null;
	return createPortal(
		<div
			className="fixed inset-0 z-50 flex items-start justify-center bg-black/50 px-4 pt-[12vh] animate-fade-in"
			onMouseDown={() => closeRef.current()}
		>
			<div
				ref={panel}
				role="dialog"
				aria-modal="true"
				aria-labelledby={labelledBy}
				tabIndex={-1}
				onMouseDown={(e) => e.stopPropagation()}
				style={{ width }}
				className="max-w-full animate-pop-in rounded-xl border border-line-strong bg-surface-2 shadow-pop"
			>
				{(title || description) && (
					<div className="flex items-start justify-between gap-4 border-b border-line px-5 py-4">
						<div>
							{title && (
								<h2 id={labelledBy} className="text-lg font-semibold text-fg">
									{title}
								</h2>
							)}
							{description && <p className="mt-1 text-base text-fg-2">{description}</p>}
						</div>
						<IconButton label="Close" size="sm" onClick={onClose} data-close>
							<X className="size-4" />
						</IconButton>
					</div>
				)}
				{children && <div className="px-5 py-4">{children}</div>}
				{footer && <div className="flex justify-end gap-2 border-t border-line px-5 py-3">{footer}</div>}
			</div>
		</div>,
		document.body,
	);
}
