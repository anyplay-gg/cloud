import { paraglideVitePlugin } from "@inlang/paraglide-js";
import tailwindcss from "@tailwindcss/vite";
import { tanstackRouter } from "@tanstack/router-plugin/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// The portal is static files the cloud serves on the same origin as /web/api/v1, so the build
// has no runtime server. `pnpm dev` talks to the in-memory mock; `pnpm dev:cloud` proxies the
// API to a cloud on this machine.
const cloud = process.env.ANYPLAY_CLOUD_URL ?? "http://127.0.0.1:8080";

export default defineConfig(({ command, mode }) => ({
	define: {
		// The mock adapter is bundled for `pnpm dev` and `pnpm build:mock` only. A production build
		// drops it, so the real bundle never carries seed data.
		__MOCK__: JSON.stringify(
			process.env.VITE_API === "http" ? false : command === "serve" || mode === "mock",
		),
	},
	plugins: [
		paraglideVitePlugin({
			project: "./project.inlang",
			outdir: "./src/paraglide",
			emitTsDeclarations: true,
			strategy: ["localStorage", "preferredLanguage", "baseLocale"],
		}),
		tanstackRouter({ target: "react", autoCodeSplitting: true }),
		react(),
		tailwindcss(),
	],
	resolve: {
		tsconfigPaths: true,
	},
	build: {
		target: "es2023",
		// CSP is `font-src 'self'`: no data: URIs, so no inlined assets.
		assetsInlineLimit: 0,
		sourcemap: false,
	},
	server: {
		port: 5173,
		proxy: {
			"/web/api": { target: cloud, changeOrigin: false },
		},
	},
	test: {
		environment: "jsdom",
		setupFiles: ["./test/setup.ts"],
		include: ["src/**/*.test.{ts,tsx}", "test/**/*.test.{ts,tsx}"],
		css: false,
		restoreMocks: true,
	},
}));
