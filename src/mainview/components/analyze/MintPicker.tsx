import { useEffect, useId, useMemo, useState, type ReactNode } from "react";
import { ArrowRight, Check, ClipboardPaste, Clock, LayoutList, Star, Zap } from "lucide-react";
import { extractMint, shortMint } from "../../../shared/analysis";
import { cn } from "../../lib/cn";
import { ago } from "../../lib/format";
import { toast, useApp } from "../../lib/store";
import { TokenAvatar } from "../token/widgets";
import { Button } from "../ui/Button";
import { Badge, FieldMessage, MOD, TextInput } from "../ui/primitives";

const EMPTY: string[] = [];

/** Empty state of the Analyze screen: paste a mint or pick a recent / watched token. */
export function MintPicker({ active }: { active: boolean }) {
	const openAnalyze = useApp((s) => s.openAnalyze);
	const setView = useApp((s) => s.setView);
	const recent = useApp((s) => s.config?.prefs.recentMints ?? EMPTY);
	const watchlist = useApp((s) => s.config?.prefs.watchlist ?? EMPTY);
	const [value, setValue] = useState("");
	const [error, setError] = useState<string | null>(null);
	const id = useId();
	const errId = `${id}-err`;

	const focusInput = () => document.getElementById(id)?.focus();

	// Coming back to the screen puts the cursor in the field, ready to paste.
	useEffect(() => {
		if (active) requestAnimationFrame(() => document.getElementById(id)?.focus({ preventScroll: true }));
	}, [active, id]);

	function load(raw: string) {
		const mint = extractMint(raw);
		if (!raw.trim()) {
			setError("Paste a mint address or a token link first");
			focusInput();
			return;
		}
		if (!mint) {
			setError("No mint address found. Use a Solana mint (32 to 44 characters) or a link that contains one.");
			focusInput();
			return;
		}
		setError(null);
		setValue("");
		openAnalyze(mint);
	}

	async function paste() {
		try {
			const text = await navigator.clipboard.readText();
			setValue(text.trim());
			if (extractMint(text)) load(text);
			else if (!text.trim()) setError("Your clipboard is empty");
			else setError("The clipboard does not contain a mint address or token link");
		} catch {
			toast({
				kind: "error",
				title: "Could not read the clipboard",
				description: `Click the field and press ${MOD}+V instead.`,
				duration: 5000,
			});
			focusInput();
		}
	}

	return (
		<div className="relative min-h-0 flex-1 overflow-y-auto">
			<div className="mx-auto flex w-full max-w-[760px] flex-col px-6 pb-12 pt-[12vh]">
				<div className="mb-6 flex items-start gap-4">
					<div className="flex size-11 shrink-0 items-center justify-center rounded-xl border border-line-strong bg-surface-2 text-brand">
						<Zap className="size-5" aria-hidden />
					</div>
					<div className="min-w-0">
						<h2 className="text-xl font-semibold text-fg">Analyze a token</h2>
						<p className="mt-0.5 text-base text-fg-3">
							Load a token, review the pre-filled labels and submit them. Validated submissions earn XP.
						</p>
					</div>
				</div>

				<form
					onSubmit={(e) => {
						e.preventDefault();
						load(value);
					}}
				>
					<label htmlFor={id} className="mb-1.5 block text-sm font-medium text-fg-2">
						Token
					</label>
					<div className="flex gap-2">
						<TextInput
							id={id}
							value={value}
							onChange={(e) => {
								setValue(e.target.value);
								if (error) setError(null);
							}}
							placeholder="Paste a mint address or a pump.fun, pump.studio, DEX Screener or Solscan link"
							invalid={!!error}
							aria-describedby={error ? errId : undefined}
							spellCheck={false}
							autoComplete="off"
							className="!h-10"
						/>
						<Button size="lg" onClick={() => void paste()} icon={<ClipboardPaste className="size-4" />}>
							Paste
						</Button>
						<Button size="lg" variant="primary" type="submit" iconRight={<ArrowRight className="size-4" />}>
							Analyze
						</Button>
					</div>
					{error && (
						<FieldMessage error id={errId}>
							{error}
						</FieldMessage>
					)}
				</form>

				<div className="mt-8 grid grid-cols-1 gap-4 lg:grid-cols-2">
					<TokenList
						title="Recent"
						icon={<Clock className="size-3.5" aria-hidden />}
						mints={recent}
						empty="Tokens you open appear here."
						onPick={openAnalyze}
					/>
					<TokenList
						title="Watchlist"
						icon={<Star className="size-3.5" aria-hidden />}
						mints={watchlist}
						empty="Star a token to keep it here."
						onPick={openAnalyze}
					/>
				</div>

				<div className="mt-6 flex items-center justify-between gap-3 rounded-lg border border-line bg-surface-1 px-4 py-3">
					<div className="min-w-0 text-base text-fg-2">
						Not sure what to pick? Browse new, live and graduated tokens.
					</div>
					<Button onClick={() => setView("tokens")} icon={<LayoutList className="size-4" />}>
						Browse tokens
					</Button>
				</div>
			</div>
		</div>
	);
}

function TokenList({
	title,
	icon,
	mints,
	empty,
	onPick,
}: {
	title: string;
	icon: ReactNode;
	mints: string[];
	empty: string;
	onPick: (mint: string) => void;
}) {
	const tokenMeta = useApp((s) => s.tokenMeta);
	const analyzed = useApp((s) => s.analyzed);
	const rows = useMemo(() => mints.slice(0, 8), [mints]);

	return (
		<section className="min-w-0 rounded-lg border border-line bg-surface-2">
			<header className="flex h-10 items-center gap-2 border-b border-line px-3 text-fg-3">
				{icon}
				<h3 className="text-caption uppercase">{title}</h3>
				{mints.length > 0 && <span className="num ml-auto text-sm text-fg-3">{mints.length}</span>}
			</header>
			{rows.length === 0 ? (
				<p className="px-3 py-6 text-center text-base text-fg-3">{empty}</p>
			) : (
				<ul className="p-1">
					{rows.map((mint) => {
						const meta = tokenMeta[mint];
						const mine = analyzed[mint];
						const label = meta?.symbol ? `$${meta.symbol}` : shortMint(mint);
						return (
							<li key={mint}>
								<button
									type="button"
									onClick={() => onPick(mint)}
									title={meta?.name ? `${meta.name} · ${mint}` : mint}
									className={cn(
										"focus-inset group flex h-11 w-full items-center gap-2.5 rounded-md px-2 text-left transition-colors duration-fast hover:bg-surface-3",
									)}
								>
									<TokenAvatar src={meta?.image} symbol={meta?.symbol} mint={mint} size={28} />
									<span className="min-w-0 flex-1">
										<span className="block truncate text-base font-medium text-fg">{label}</span>
										<span className="block truncate text-sm text-fg-3">
											{meta?.name ?? <span className="font-mono">{shortMint(mint, 6, 6)}</span>}
										</span>
									</span>
									{mine && (
										<Badge
											tone={mine.validated ? "pos" : "neutral"}
											icon={<Check className="size-3" aria-hidden />}
											title={`You analyzed this ${ago(mine.at)}`}
										>
											Analyzed
										</Badge>
									)}
									<ArrowRight
										className="size-4 shrink-0 text-fg-3 opacity-0 transition-opacity group-hover:opacity-100 group-focus-visible:opacity-100"
										aria-hidden
									/>
								</button>
							</li>
						);
					})}
				</ul>
			)}
		</section>
	);
}
