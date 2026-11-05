/**
 * Type-check the view and the Bun process.
 *
 * Electrobun ships TypeScript sources, so tsc also checks them and reports a few
 * errors inside node_modules/electrobun that skipLibCheck cannot hide. Those are
 * ignored here; any error in our own code fails the check.
 */
import { spawnSync } from "node:child_process";

let failed = false;
for (const project of ["tsconfig.json", "tsconfig.bun.json"]) {
	const res = spawnSync("npx", ["tsc", "-p", project, "--pretty", "false"], { encoding: "utf8" });
	const lines = `${res.stdout}${res.stderr}`.split("\n").filter(Boolean);
	const errors = lines.filter((l) => /error TS\d+/.test(l) && !l.startsWith("node_modules/"));
	if (errors.length) {
		failed = true;
		console.error(`✗ ${project}\n${errors.join("\n")}`);
	} else {
		console.log(`✓ ${project}`);
	}
}
process.exit(failed ? 1 : 0);
