import {
	forwardRef,
	useId,
	useRef,
	type CSSProperties,
	type InputHTMLAttributes,
	type KeyboardEvent,
	type ReactNode,
	type TextareaHTMLAttributes,
} from "react";
import { AlertTriangle, CheckCircle2, Info, RotateCw, XCircle } from "lucide-react";
import { cn } from "../../lib/cn";
import { Button } from "./Button";

/* ---- Badge ---------------------------------------------------------- */

export type Tone = "neutral" | "pos" | "neg" | "warn" | "info" | "violet" | "orange" | "brand";

const TONES: Record<Tone, string> = {
	neutral: "bg-surface-3 text-fg-2 border-line-strong",
	pos: "bg-pos/10 text-pos border-pos/25",
	neg: "bg-neg/10 text-neg border-neg/25",
	warn: "bg-warn/10 text-warn border-warn/25",
	info: "bg-info/10 text-info border-info/25",
	violet: "bg-violet/10 text-violet border-violet/25",
	orange: "bg-orange/10 text-orange border-orange/25",
	brand: "bg-brand/10 text-brand border-brand/25",
};

export function Badge({
	tone = "neutral",
	icon,
	children,
	className,
	title,
	dot,
	truncate,
}: {
	tone?: Tone;
	icon?: ReactNode;
	children: ReactNode;
	className?: string;
	title?: string;
	dot?: boolean;
	/** Allow the badge to shrink and truncate inside flex rows. */
	truncate?: boolean;
}) {
	return (
		<span
			title={title}
			className={cn(
				"inline-flex h-5 items-center gap-1 whitespace-nowrap rounded border px-1.5 text-[11px] font-medium leading-none",
				truncate ? "min-w-0 max-w-full" : "shrink-0",
				TONES[tone],
				className,
			)}
		>
			{dot && <span className="size-1.5 shrink-0 rounded-full bg-current" aria-hidden />}
			{icon}
			{truncate ? <span className="min-w-0 truncate">{children}</span> : children}
		</span>
	);
}

/* ---- Kbd ------------------------------------------------------------ */

export function Kbd({ children, className }: { children: ReactNode; className?: string }) {
	return (
		<kbd
			className={cn(
				"inline-flex h-5 min-w-5 items-center justify-center rounded border border-line-strong bg-surface-2 px-1 font-sans text-[11px] font-medium text-fg-3",
				className,
			)}
		>
			{children}
		</kbd>
	);
}

export const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
export const MOD = isMac ? "⌘" : "Ctrl";

/* ---- Segmented control (radiogroup with roving tabindex) ------------- */

export interface SegmentOption<T extends string> {
	value: T;
	label: ReactNode;
	tone?: Tone;
	title?: string;
}

export function Segmented<T extends string>({
	value,
	options,
	onChange,
	label,
	size = "md",
	className,
	full,
}: {
	value: T;
	options: readonly SegmentOption<T>[];
	onChange: (v: T) => void;
	label: string;
	size?: "sm" | "md";
	className?: string;
	full?: boolean;
}) {
	const refs = useRef<Array<HTMLButtonElement | null>>([]);
	const idx = Math.max(
		0,
		options.findIndex((o) => o.value === value),
	);

	function onKey(e: KeyboardEvent) {
		let next = -1;
		if (e.key === "ArrowRight" || e.key === "ArrowDown") next = (idx + 1) % options.length;
		if (e.key === "ArrowLeft" || e.key === "ArrowUp") next = (idx - 1 + options.length) % options.length;
		if (e.key === "Home") next = 0;
		if (e.key === "End") next = options.length - 1;
		if (next >= 0) {
			e.preventDefault();
			onChange(options[next]!.value);
			refs.current[next]?.focus();
		}
	}

	const toneText: Record<Tone, string> = {
		neutral: "text-fg",
		pos: "text-pos",
		neg: "text-neg",
		warn: "text-warn",
		info: "text-info",
		violet: "text-violet",
		orange: "text-orange",
		brand: "text-brand",
	};

	return (
		<div
			role="radiogroup"
			aria-label={label}
			onKeyDown={onKey}
			className={cn(
				"inline-flex rounded-md border border-line bg-surface-1 p-0.5",
				full && "flex w-full",
				className,
			)}
		>
			{options.map((o, i) => {
				const selected = o.value === value;
				return (
					<button
						key={o.value}
						ref={(el) => {
							refs.current[i] = el;
						}}
						type="button"
						role="radio"
						aria-checked={selected}
						tabIndex={selected ? 0 : -1}
						title={o.title}
						onClick={() => onChange(o.value)}
						className={cn(
							"focus-inset inline-flex flex-1 items-center justify-center gap-1.5 whitespace-nowrap rounded font-medium transition-colors duration-fast",
							size === "sm" ? "h-6 px-2 text-sm" : "h-7 px-3 text-base",
							selected
								? cn("bg-surface-4 shadow-card", toneText[o.tone ?? "neutral"])
								: "text-fg-3 hover:text-fg",
						)}
					>
						{o.label}
					</button>
				);
			})}
		</div>
	);
}

/* ---- Switch --------------------------------------------------------- */

export function Switch({
	checked,
	onChange,
	label,
	description,
	disabled,
}: {
	checked: boolean;
	onChange: (v: boolean) => void;
	label: string;
	description?: ReactNode;
	disabled?: boolean;
}) {
	const id = useId();
	return (
		<div className="flex items-start justify-between gap-4">
			<div className="min-w-0">
				<label htmlFor={id} className="block text-base font-medium text-fg">
					{label}
				</label>
				{description && <div className="mt-0.5 text-sm text-fg-3">{description}</div>}
			</div>
			<button
				id={id}
				type="button"
				role="switch"
				aria-checked={checked}
				disabled={disabled}
				onClick={() => onChange(!checked)}
				className={cn(
					"relative mt-0.5 inline-flex h-5 w-9 shrink-0 items-center rounded-full border transition-colors duration-fast disabled:opacity-50",
					checked ? "border-brand bg-brand" : "border-line-control bg-surface-3",
				)}
			>
				<span
					className={cn(
						"block size-3.5 rounded-full shadow-card transition-transform duration-fast",
						checked ? "translate-x-[18px] bg-brand-fg" : "translate-x-[3px] bg-fg-2",
					)}
				/>
			</button>
		</div>
	);
}

/* ---- Form fields ---------------------------------------------------- */

export function FieldLabel({
	htmlFor,
	children,
	hint,
	id,
}: {
	htmlFor?: string;
	children: ReactNode;
	hint?: ReactNode;
	id?: string;
}) {
	return (
		<div className="mb-1.5 flex items-baseline justify-between gap-2">
			<label id={id} htmlFor={htmlFor} className="text-sm font-medium text-fg-2">
				{children}
			</label>
			{hint && <span className="text-sm text-fg-3">{hint}</span>}
		</div>
	);
}

export function FieldMessage({ error, children, id }: { error?: boolean; children: ReactNode; id?: string }) {
	return (
		<p id={id} className={cn("mt-1.5 text-sm", error ? "text-neg" : "text-fg-3")} role={error ? "alert" : undefined}>
			{children}
		</p>
	);
}

export const TextInput = forwardRef<HTMLInputElement, InputHTMLAttributes<HTMLInputElement> & { invalid?: boolean; mono?: boolean }>(
	function TextInput({ className, invalid, mono, ...rest }, ref) {
		return (
			<input
				ref={ref}
				aria-invalid={invalid || undefined}
				className={cn("field h-8 w-full px-2.5 text-base", mono && "font-mono text-sm", className)}
				{...rest}
			/>
		);
	},
);

export const TextArea = forwardRef<HTMLTextAreaElement, TextareaHTMLAttributes<HTMLTextAreaElement> & { invalid?: boolean }>(
	function TextArea({ className, invalid, ...rest }, ref) {
		return (
			<textarea
				ref={ref}
				aria-invalid={invalid || undefined}
				className={cn("field w-full resize-none px-2.5 py-2 text-base leading-relaxed", className)}
				{...rest}
			/>
		);
	},
);

/* ---- Slider + number for 0-100 values -------------------------------- */

export function RangeField({
	label,
	value,
	onChange,
	min = 0,
	max = 100,
	hint,
	valueText,
	fill,
	reference,
	error,
}: {
	label: string;
	value: number;
	onChange: (v: number) => void;
	min?: number;
	max?: number;
	hint?: ReactNode;
	valueText?: string;
	/** CSS color for the filled part of the track. */
	fill?: string;
	/** Computed reference value, shown as a hint the user can click to restore. */
	reference?: number;
	error?: string;
}) {
	const id = useId();
	const pctPos = ((value - min) / (max - min)) * 100;
	const clampN = (n: number) => Math.max(min, Math.min(max, Math.round(n)));
	return (
		<div>
			<FieldLabel
				htmlFor={id}
				hint={
					reference != null && reference !== value ? (
						<button
							type="button"
							className="rounded text-sm text-fg-3 underline decoration-dotted underline-offset-2 hover:text-fg"
							onClick={() => onChange(reference)}
							title="Use the value computed from the snapshot"
						>
							Data says {reference}
						</button>
					) : (
						hint
					)
				}
			>
				{label}
			</FieldLabel>
			<div className="flex items-center gap-3">
				<input
					id={id}
					type="range"
					min={min}
					max={max}
					step={1}
					value={value}
					aria-valuetext={valueText ?? String(value)}
					onChange={(e) => onChange(clampN(Number(e.target.value)))}
					className="range flex-1"
					style={{ ["--range-pct" as string]: `${pctPos}%`, ...(fill ? { ["--range-fill" as string]: fill } : {}) }}
				/>
				<input
					type="number"
					inputMode="numeric"
					aria-label={`${label} value`}
					min={min}
					max={max}
					value={Number.isFinite(value) ? value : ""}
					onChange={(e) => {
						if (e.target.value === "") return;
						onChange(clampN(Number(e.target.value)));
					}}
					aria-invalid={!!error || undefined}
					className="field num h-8 w-14 px-2 text-center text-base"
				/>
			</div>
			{error && <FieldMessage error>{error}</FieldMessage>}
		</div>
	);
}

/* ---- Skeleton -------------------------------------------------------- */

export function Skeleton({ className, style }: { className?: string; style?: CSSProperties }) {
	return <div aria-hidden style={style} className={cn("skeleton skeleton-shimmer", className)} />;
}

/* ---- States ---------------------------------------------------------- */

export function EmptyState({
	icon,
	title,
	description,
	action,
	className,
}: {
	icon?: ReactNode;
	title: string;
	description?: ReactNode;
	action?: ReactNode;
	className?: string;
}) {
	return (
		<div className={cn("flex flex-col items-center justify-center px-6 py-12 text-center", className)}>
			{icon && (
				<div className="mb-4 flex size-11 items-center justify-center rounded-xl border border-line-strong bg-surface-2 text-fg-3">
					{icon}
				</div>
			)}
			<h3 className="text-md font-semibold text-fg">{title}</h3>
			{description && <p className="mt-1 max-w-sm text-base text-fg-3">{description}</p>}
			{action && <div className="mt-5 flex items-center gap-2">{action}</div>}
		</div>
	);
}

export function ErrorState({ message, onRetry, title = "Could not load data" }: { message: string; onRetry?: () => void; title?: string }) {
	return (
		<EmptyState
			icon={<XCircle className="size-5 text-neg" />}
			title={title}
			description={message}
			action={
				onRetry && (
					<Button onClick={onRetry} icon={<RotateCw className="size-4" />}>
						Try again
					</Button>
				)
			}
		/>
	);
}

export function Banner({
	tone = "info",
	title,
	children,
	action,
	className,
}: {
	tone?: "info" | "warn" | "neg" | "pos";
	title?: ReactNode;
	children?: ReactNode;
	action?: ReactNode;
	className?: string;
}) {
	const Icon = tone === "neg" ? XCircle : tone === "warn" ? AlertTriangle : tone === "pos" ? CheckCircle2 : Info;
	const color = { info: "text-info", warn: "text-warn", neg: "text-neg", pos: "text-pos" }[tone];
	const bg = { info: "bg-info/5 border-info/25", warn: "bg-warn/5 border-warn/25", neg: "bg-neg/5 border-neg/25", pos: "bg-pos/5 border-pos/25" }[tone];
	return (
		<div role={tone === "neg" ? "alert" : "status"} className={cn("flex items-start gap-3 rounded-lg border px-3 py-2.5", bg, className)}>
			<Icon className={cn("mt-0.5 size-4 shrink-0", color)} aria-hidden />
			<div className="min-w-0 flex-1 text-base">
				{title && <div className="font-medium text-fg">{title}</div>}
				{children && <div className={cn("text-fg-2", title && "mt-0.5")}>{children}</div>}
			</div>
			{action && <div className="shrink-0">{action}</div>}
		</div>
	);
}

/* ---- Layout bits ----------------------------------------------------- */

export function Card({ children, className, as: As = "div" }: { children: ReactNode; className?: string; as?: "div" | "section" }) {
	return <As className={cn("rounded-lg border border-line bg-surface-2", className)}>{children}</As>;
}

export function SectionTitle({ children, action, className }: { children: ReactNode; action?: ReactNode; className?: string }) {
	return (
		<div className={cn("mb-3 flex items-center justify-between gap-2", className)}>
			<h3 className="text-caption uppercase text-fg-3">{children}</h3>
			{action}
		</div>
	);
}

export function Stat({
	label,
	value,
	sub,
	tone,
	className,
	title,
}: {
	label: ReactNode;
	value: ReactNode;
	sub?: ReactNode;
	tone?: "pos" | "neg" | "warn" | "brand";
	className?: string;
	title?: string;
}) {
	const color = tone ? { pos: "text-pos", neg: "text-neg", warn: "text-warn", brand: "text-brand" }[tone] : "text-fg";
	return (
		<div className={cn("min-w-0", className)} title={title}>
			<div className="truncate text-caption uppercase text-fg-3">{label}</div>
			<div className={cn("num mt-0.5 truncate text-lg font-semibold", color)}>{value}</div>
			{sub && <div className="num truncate text-sm text-fg-3">{sub}</div>}
		</div>
	);
}
