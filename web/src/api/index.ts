import { createHttpApi } from "./http-adapter";
import type { PortalApi } from "./portal-api";

export { ApiError, describeError, isApiError, isStepUp } from "./errors";
export type { PortalApi } from "./portal-api";
export * from "./types";

declare const __MOCK__: boolean;

/**
 * The adapter for this build. `pnpm dev` and `pnpm build:mock` use the in-memory mock (add
 * `?mock=empty|error|slow|signedout` to the URL to see a state); every other build, and
 * `pnpm dev:cloud`, talks to the cloud on the same origin.
 */
export async function createDefaultApi(): Promise<PortalApi> {
	if (__MOCK__) {
		const { createMockApi, parseScenario } = await import("./mock/adapter");
		const key = "anyplay.mock";
		let scenario = parseScenario(window.location.search);
		try {
			if (scenario !== "default") sessionStorage.setItem(key, scenario);
			else if (!window.location.search.includes("mock=")) {
				scenario = parseScenario(`?mock=${sessionStorage.getItem(key) ?? ""}`);
			} else sessionStorage.removeItem(key);
		} catch {
			// storage can be blocked; the scenario then lasts for this page load
		}
		return createMockApi({ scenario });
	}
	return createHttpApi();
}
