// Shared by `pnpm shots` and `pnpm a11y`: serve the mock build, drive the installed Chrome.
import { existsSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";
import { build, preview } from "vite";

export const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

const CHROME = process.env.CHROME_PATH ?? "C:/Program Files/Google/Chrome/Application/chrome.exe";

// The policy the cloud serves the portal under. A violation shows up as a console error, which
// `open()` collects, so a script, style or font the CSP would block fails the run.
const CSP =
	"default-src 'none'; script-src 'self'; style-src 'self'; font-src 'self'; img-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

/** Builds the mock bundle unless `--no-build`, serves it, and returns what a script needs. */
export async function start({ port = 4173 } = {}) {
	if (!process.argv.includes("--no-build") || !existsSync(resolve(root, "dist-mock"))) {
		await build({
			root,
			mode: "mock",
			logLevel: "warn",
			build: { outDir: "dist-mock", emptyOutDir: true },
		});
	}
	const server = await preview({
		root,
		build: { outDir: "dist-mock" },
		preview: { port, strictPort: true, headers: { "Content-Security-Policy": CSP } },
		logLevel: "error",
	});
	const browser = await chromium.launch({ executablePath: CHROME, headless: true });
	return {
		base: `http://localhost:${port}`,
		browser,
		async stop() {
			await browser.close();
			await new Promise((r) => server.httpServer.close(r));
		},
	};
}

/** Opens one screen in a fresh context and waits until it shows what it should. */
export async function open(browser, base, screen, { scheme, viewport }) {
	const context = await browser.newContext({
		viewport,
		colorScheme: scheme,
		deviceScaleFactor: 1,
		reducedMotion: "reduce",
		hasTouch: viewport.width < 600,
	});
	const page = await context.newPage();
	const problems = [];
	page.on("pageerror", (e) => problems.push(`pageerror: ${e.message}`));
	page.on("console", (m) => {
		if (m.type() === "error") problems.push(`console: ${m.text()}`);
	});
	await page.goto(base + screen.path, { waitUntil: "networkidle" });
	await page.getByText(screen.ready, { exact: false }).first().waitFor({ timeout: 8000 });
	if (screen.act) await screen.act(page);
	await page.waitForTimeout(screen.settle ?? 700);
	return { page, context, problems };
}
