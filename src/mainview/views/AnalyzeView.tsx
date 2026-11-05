import { PageHeader } from "../components/shell/chrome";
import { AnalyzeWorkspace } from "../components/analyze/AnalyzeWorkspace";
import { MintPicker } from "../components/analyze/MintPicker";
import { useApp } from "../lib/store";

export function AnalyzeView({ active }: { active: boolean }) {
	const mint = useApp((s) => s.analyzeMint);
	const symbol = useApp((s) => (s.analyzeMint ? s.tokenMeta[s.analyzeMint]?.symbol : undefined));

	return (
		<>
			<PageHeader
				title="Analyze"
				description={
					mint
						? symbol
							? `Label $${symbol} and submit it for XP`
							: "Label this token and submit it for XP"
						: "Label tokens and earn XP for validated submissions"
				}
			/>
			{mint ? <AnalyzeWorkspace key={mint} mint={mint} active={active} /> : <MintPicker active={active} />}
		</>
	);
}
