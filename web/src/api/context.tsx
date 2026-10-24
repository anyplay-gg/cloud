import { createContext, type ReactNode, useContext } from "react";
import type { PortalApi } from "./portal-api";

const ApiContext = createContext<PortalApi | null>(null);

export function ApiProvider({ api, children }: { api: PortalApi; children: ReactNode }) {
	return <ApiContext.Provider value={api}>{children}</ApiContext.Provider>;
}

/** The adapter chosen at start-up (or by a test). Screens never import an adapter directly. */
export function useApi(): PortalApi {
	const api = useContext(ApiContext);
	if (!api) throw new Error("useApi needs <ApiProvider>");
	return api;
}
