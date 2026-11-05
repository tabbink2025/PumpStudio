import { forwardRef, type ButtonHTMLAttributes, type ReactNode } from "react";
import { Loader2 } from "lucide-react";
import { cn } from "../../lib/cn";

type Variant = "primary" | "secondary" | "ghost" | "danger" | "ai";
type Size = "sm" | "md" | "lg";

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
	variant?: Variant;
	size?: Size;
	icon?: ReactNode;
	iconRight?: ReactNode;
	loading?: boolean;
	/** Keyboard hint shown at the right edge, e.g. "⌘↵". */
	kbd?: string;
}

const VARIANTS: Record<Variant, string> = {
	primary: "bg-brand text-brand-fg hover:bg-brand-hover font-semibold disabled:hover:bg-brand",
	secondary: "bg-surface-3 text-fg border border-line-strong hover:bg-surface-4 hover:border-line-control",
	ghost: "text-fg-2 hover:text-fg hover:bg-surface-3",
	danger: "bg-neg/10 text-neg border border-neg/30 hover:bg-neg/20",
	ai: "bg-violet/10 text-violet border border-violet/30 hover:bg-violet/20",
};

const SIZES: Record<Size, string> = {
	sm: "h-7 px-2.5 gap-1.5 text-sm rounded",
	md: "h-8 px-3 gap-2 text-base rounded-md",
	lg: "h-10 px-4 gap-2 text-md rounded-lg",
};

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
	{ variant = "secondary", size = "md", icon, iconRight, loading, kbd, className, children, disabled, type = "button", ...rest },
	ref,
) {
	return (
		<button
			ref={ref}
			type={type}
			disabled={disabled || loading}
			aria-busy={loading || undefined}
			className={cn(
				"inline-flex shrink-0 select-none items-center justify-center whitespace-nowrap font-medium",
				"transition-colors duration-fast ease-out disabled:cursor-not-allowed disabled:opacity-50",
				VARIANTS[variant],
				SIZES[size],
				className,
			)}
			{...rest}
		>
			{loading ? <Loader2 className="size-4 animate-spin" aria-hidden /> : icon}
			{children}
			{iconRight}
			{kbd && (
				<kbd
					className={cn(
						"ml-1 rounded px-1 font-sans text-[11px] leading-4",
						variant === "primary" ? "bg-black/15 text-brand-fg/80" : "bg-surface-1 text-fg-3",
					)}
				>
					{kbd}
				</kbd>
			)}
		</button>
	);
});

export interface IconButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
	label: string;
	size?: "sm" | "md";
	active?: boolean;
}

export const IconButton = forwardRef<HTMLButtonElement, IconButtonProps>(function IconButton(
	{ label, size = "md", active, className, children, type = "button", ...rest },
	ref,
) {
	return (
		<button
			ref={ref}
			type={type}
			aria-label={label}
			title={label}
			className={cn(
				"inline-flex shrink-0 items-center justify-center rounded-md text-fg-2 transition-colors duration-fast",
				"hover:bg-surface-3 hover:text-fg disabled:cursor-not-allowed disabled:opacity-50",
				size === "sm" ? "size-7" : "size-8",
				active && "bg-surface-3 text-fg",
				className,
			)}
			{...rest}
		>
			{children}
		</button>
	);
});
