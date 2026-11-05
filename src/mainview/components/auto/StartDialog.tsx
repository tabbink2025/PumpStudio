import { useCallback, useId, useRef } from "react";
import { AlertTriangle, Globe, Play } from "lucide-react";
import { shortMint } from "../../../shared/analysis";
import type { PublicConfig } from "../../../shared/types";
import { integer, plural } from "../../lib/format";
import { TokenAvatar } from "../token/widgets";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/overlays";
import { Banner } from "../ui/primitives";
import { about, engineOf, estimateSec, MAX_TARGETS, type PickToken } from "./model";

const PREVIEW = 14;

function Row({ label, children, title }: { label: string; children: React.ReactNode; title?: string }) {
	return (
		<div className="flex items-baseline justify-between gap-4 py-1.5" title={title}>
			<dt className="shrink-0 text-sm text-fg-3">{label}</dt>
			<dd className="min-w-0 truncate text-right text-base text-fg">{children}</dd>
		</div>
	);
}

export function StartDialog({
	targets,
	retry,
	config,
	starting,
	error,
	onClose,
	onConfirm,
}: {
	targets: PickToken[] | null;
	retry?: boolean;
	config: PublicConfig | null;
	starting: boolean;
	error?: string;
	onClose: () => void;
	onConfirm: () => void;
}) {
	const titleId = useId();
	// Dialog re-runs its focus effect when onClose changes, so keep it stable.
	const latest = useRef({ onClose, starting });
	latest.current = { onClose, starting };
	const close = useCallback(() => {
		if (!latest.current.starting) latest.current.onClose();
	}, []);
	const list = targets ?? [];
	const count = Math.min(list.length, MAX_TARGETS);
	const engine = engineOf(config);
	const ai = engine.ai && !engine.missingKey;
	const interval = config?.analyzeInterval ?? 10;

	return (
		<Dialog
			open={targets != null}
			onClose={close}
			labelledBy={titleId}
			width={520}
			title={`Submit ${plural(count, "analysis", "analyses")}?`}
			description={
				retry
					? "Runs the tokens that failed, were skipped or did not run in the last batch."
					: "Your agent analyzes each token and submits the result to pump.studio."
			}
			footer={
				<>
					<Button variant="ghost" onClick={close} disabled={starting}>
						Cancel
					</Button>
					<Button variant="primary" data-autofocus icon={<Play className="size-4" />} loading={starting} onClick={onConfirm}>
						Start training
					</Button>
				</>
			}
		>
			<div className="space-y-4">
				<div className="flex flex-wrap gap-1.5" aria-label="Tokens in this run">
					{list.slice(0, PREVIEW).map((t) => (
						<span
							key={t.mint}
							title={t.name ? `$${t.symbol} · ${t.name}` : t.mint}
							className="inline-flex h-6 max-w-[132px] items-center gap-1.5 rounded-full border border-line bg-surface-3 pl-0.5 pr-2 text-sm text-fg-2"
						>
							<TokenAvatar mint={t.mint} src={t.image} symbol={t.symbol} size={18} />
							<span className="truncate">{t.symbol ? `$${t.symbol}` : shortMint(t.mint)}</span>
						</span>
					))}
					{list.length > PREVIEW && (
						<span className="num inline-flex h-6 items-center rounded-full border border-line px-2 text-sm text-fg-3">
							+{integer(list.length - PREVIEW)} more
						</span>
					)}
				</div>

				<dl className="divide-y divide-line rounded-lg border border-line bg-surface-1 px-3">
					<Row label="Tokens"><span className="num">{integer(count)}</span></Row>
					<Row label="Interval">Every {interval} s</Row>
					<Row label="Estimated time">{about(estimateSec(count, interval, ai))}</Row>
					<Row label="Engine" title={engine.label}>
						{engine.missingKey ? "Rules" : engine.label}
						{ai && <span className="text-fg-3"> · up to {plural(count, "AI call")}</span>}
						{engine.missingKey && <span className="text-fg-3"> · no {engine.provider} key</span>}
					</Row>
					<Row label="Skip recent">{config?.skipRecentlyAnalyzed ? "On, skips tokens analyzed in the last 2 min" : "Off"}</Row>
				</dl>

				{list.length > MAX_TARGETS && (
					<p className="flex items-start gap-1.5 text-sm text-warn">
						<AlertTriangle className="mt-0.5 size-3.5 shrink-0" aria-hidden />
						Only the first {MAX_TARGETS} of {integer(list.length)} tokens run in one batch.
					</p>
				)}
				{engine.missingKey && (
					<p className="flex items-start gap-1.5 text-sm text-warn">
						<AlertTriangle className="mt-0.5 size-3.5 shrink-0" aria-hidden />
						No {engine.provider} API key is set, so every token uses rule-based analysis.
					</p>
				)}
				<p className="flex items-start gap-2 text-sm text-fg-3">
					<Globe className="mt-0.5 size-3.5 shrink-0" aria-hidden />
					Each submission is public in the Pump Studio dataset. The run continues if you switch screens.
				</p>
				{error && (
					<Banner tone="neg" title="Could not start training">
						{error}
					</Banner>
				)}
			</div>
		</Dialog>
	);
}
