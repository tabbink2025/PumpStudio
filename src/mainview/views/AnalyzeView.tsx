import { PageHeader } from "../components/shell/chrome";

export function AnalyzeView({ active }: { active: boolean }) {
	void active;
	return <PageHeader title="Analyze" />;
}
