import type { QueryClient } from "@tanstack/react-query";
import { createRouter } from "@tanstack/react-router";
import type { PortalApi } from "@/api/portal-api";
import { routeTree } from "./routeTree.gen";

export interface RouterContext {
	queryClient: QueryClient;
	api: PortalApi;
}

export function createAppRouter(
	context: RouterContext,
	history?: Parameters<typeof createRouter>[0]["history"],
) {
	return createRouter({
		routeTree,
		context,
		history,
		defaultPreload: false,
		scrollRestoration: true,
		defaultStructuralSharing: true,
	});
}

declare module "@tanstack/react-router" {
	interface Register {
		router: ReturnType<typeof createAppRouter>;
	}
}
