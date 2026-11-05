import { PageHeader } from "../components/shell/chrome";

export function SettingsView({ active }: { active: boolean }) {
	void active;
	return <PageHeader title="Settings" />;
}
