// Screenshots of every screen: dark and light, desktop and phone, from the mock build in the
// installed Chrome. `pnpm shots [out-dir] [--no-build] [--only=id,id]`. Not part of the repo.
import { mkdirSync } from "node:fs";
import { resolve } from "node:path";
import { open, start } from "./lib/browser.mjs";
import { SCREENS, VIEWPORTS } from "./lib/screens.mjs";

const out = resolve(
	process.argv.slice(2).find((a) => !a.startsWith("--")) ??
		"C:/Users/alec/Desktop/AnyPlay/notes/portal-screens",
);
const only = process.argv
	.find((a) => a.startsWith("--only="))
	?.slice(7)
	.split(",");
mkdirSync(out, { recursive: true });

const app = await start();
let n = 0;
let failed = 0;
for (const [i, screen] of SCREENS.entries()) {
	if (only && !only.includes(screen.id)) continue;
	for (const scheme of ["dark", "light"]) {
		for (const [vp, viewport] of Object.entries(VIEWPORTS)) {
			const file = `${String(i + 1).padStart(2, "0")}-${screen.id}-${scheme}-${vp}.png`;
			try {
				const { page, context, problems } = await open(app.browser, app.base, screen, {
					scheme,
					viewport,
				});
				await page.screenshot({ path: resolve(out, file), fullPage: !screen.act });
				await context.close();
				n++;
				for (const p of problems) console.log(`  ${file}: ${p}`);
			} catch (e) {
				failed++;
				console.log(`FAILED ${file}: ${String(e.message).split("\n")[0]}`);
			}
		}
	}
}
await app.stop();
console.log(`${n} screenshots in ${out}${failed ? `, ${failed} failed` : ""}`);
process.exit(failed ? 1 : 0);
