/** @type {import('tailwindcss').Config} */

/* Colors resolve to CSS variables (see src/mainview/styles/tokens.css),
   so light/dark and density switch without touching components. */
const v = (name) => `rgb(var(--${name}) / <alpha-value>)`;

export default {
	content: ["./src/mainview/**/*.{html,ts,tsx}"],
	darkMode: ["selector", '[data-theme="dark"]'],
	theme: {
		extend: {
			colors: {
				bg: v("bg"),
				surface: { 1: v("surface-1"), 2: v("surface-2"), 3: v("surface-3"), 4: v("surface-4") },
				line: { DEFAULT: v("border-subtle"), strong: v("border-strong"), control: v("border-control") },
				fg: { DEFAULT: v("text-primary"), 2: v("text-secondary"), 3: v("text-tertiary"), disabled: v("text-disabled") },
				brand: { DEFAULT: v("brand"), hover: v("brand-hover"), fg: v("brand-fg"), soft: v("brand-soft") },
				pos: v("pos"),
				neg: v("neg"),
				warn: v("warn"),
				info: v("info"),
				orange: v("orange"),
				violet: v("violet"),
				focus: v("focus"),
			},
			fontFamily: {
				sans: ['"Inter Variable"', "Inter", "system-ui", "-apple-system", "BlinkMacSystemFont", '"Segoe UI"', "sans-serif"],
				mono: ['"JetBrains Mono Variable"', '"JetBrains Mono"', "ui-monospace", '"SF Mono"', "Menlo", "Consolas", "monospace"],
			},
			fontSize: {
				caption: ["11px", { lineHeight: "16px", letterSpacing: "0.04em", fontWeight: "500" }],
				sm: ["12px", { lineHeight: "16px" }],
				base: ["13px", { lineHeight: "20px" }],
				md: ["14px", { lineHeight: "20px" }],
				lg: ["16px", { lineHeight: "24px", letterSpacing: "-0.01em" }],
				xl: ["20px", { lineHeight: "28px", letterSpacing: "-0.015em" }],
				kpi: ["24px", { lineHeight: "32px", letterSpacing: "-0.02em" }],
				hero: ["40px", { lineHeight: "44px", letterSpacing: "-0.03em" }],
			},
			borderRadius: {
				sm: "4px",
				DEFAULT: "6px",
				md: "6px",
				lg: "8px",
				xl: "12px",
			},
			boxShadow: {
				pop: "0 8px 24px rgb(0 0 0 / var(--shadow-alpha)), 0 0 0 1px rgb(var(--border-strong))",
				card: "0 1px 2px rgb(0 0 0 / calc(var(--shadow-alpha) * 0.5))",
			},
			spacing: {
				row: "var(--row-h)",
			},
			transitionDuration: {
				fast: "120ms",
				base: "180ms",
				slow: "240ms",
			},
			transitionTimingFunction: {
				out: "cubic-bezier(0.2, 0, 0, 1)",
			},
			keyframes: {
				"fade-in": { from: { opacity: "0" }, to: { opacity: "1" } },
				"pop-in": { from: { opacity: "0", transform: "translateY(4px) scale(0.98)" }, to: { opacity: "1", transform: "none" } },
				"slide-in": { from: { opacity: "0", transform: "translateX(16px)" }, to: { opacity: "1", transform: "none" } },
				"toast-in": { from: { opacity: "0", transform: "translateY(8px)" }, to: { opacity: "1", transform: "none" } },
				pulse: { "0%, 100%": { opacity: "1" }, "50%": { opacity: "0.45" } },
				shimmer: { "0%": { backgroundPosition: "-200% 0" }, "100%": { backgroundPosition: "200% 0" } },
				"xp-pop": {
					"0%": { opacity: "0", transform: "translateY(4px)" },
					"20%": { opacity: "1", transform: "none" },
					"80%": { opacity: "1" },
					"100%": { opacity: "0", transform: "translateY(-6px)" },
				},
			},
			animation: {
				"fade-in": "fade-in 180ms cubic-bezier(0.2,0,0,1)",
				"pop-in": "pop-in 180ms cubic-bezier(0.2,0,0,1)",
				"slide-in": "slide-in 240ms cubic-bezier(0.2,0,0,1)",
				"toast-in": "toast-in 180ms cubic-bezier(0.2,0,0,1)",
				pulse: "pulse 1.6s ease-in-out infinite",
				shimmer: "shimmer 1.6s linear infinite",
				"xp-pop": "xp-pop 2.4s cubic-bezier(0.2,0,0,1) forwards",
			},
		},
	},
	plugins: [],
};
