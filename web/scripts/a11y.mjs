// axe-core in real Chrome against the mock build, every screen, dark and light, desktop and
// phone, with colour contrast on (jsdom cannot measure it). Exits 1 on any violation.
// `pnpm a11y [--no-build] [--only=id,id]`
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { open, start } from "./lib/browser.mjs";
import { SCREENS, VIEWPORTS } from "./lib/screens.mjs";

const require = createRequire(import.meta.url);
const axeSource = readFileSync(require.resolve("axe-core/axe.min.js"), "utf8");
const only = process.argv
	.find((a) => a.startsWith("--only="))
	?.slice(7)
	.split(",");

const app = await start();
let checked = 0;
let bad = 0;
for (const screen of SCREENS) {
	if (only && !only.includes(screen.id)) continue;
	// the loading screen is a skeleton on purpose; its contrast is not the thing to measure
	for (const scheme of ["dark", "light"]) {
		for (const [vp, viewport] of Object.entries(VIEWPORTS)) {
			const label = `${screen.id} / ${scheme} / ${vp}`;
			try {
				const { page, context, problems } = await open(app.browser, app.base, screen, {
					scheme,
					viewport,
				});
				await page.evaluate(axeSource);
				const results = await page.evaluate(() =>
					axe.run(document, {
						runOnly: {
							type: "tag",
							values: ["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa", "best-practice"],
						},
						rules: { region: { enabled: false } },
					}),
				);
				checked++;
				for (const p of problems) {
					bad++;
					console.log(`${label}: ${p}`);
				}
				for (const v of results.violations) {
					bad++;
					console.log(`${label}: ${v.id} (${v.impact}) ${v.help}`);
					for (const node of v.nodes.slice(0, 3)) {
						console.log(
							`    ${node.target.join(" ")}  ${(node.any[0]?.message ?? node.failureSummary ?? "").split("\n")[0]}`,
						);
					}
				}
				await context.close();
			} catch (e) {
				bad++;
				console.log(`${label}: could not run: ${String(e.message).split("\n")[0]}`);
			}
		}
	}
}
await app.stop();
console.log(`${checked} pages checked, ${bad} problems`);
process.exit(bad ? 1 : 0);
