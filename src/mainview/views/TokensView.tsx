import { PageHeader } from "../components/shell/chrome";

export function TokensView({ active }: { active: boolean }) {
	void active;
	return <PageHeader title="Tokens" />;
}
