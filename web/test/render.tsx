import { CSPProvider } from "@base-ui/react/csp-provider";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { render, screen } from "@testing-library/react";
import { domAnimation, LazyMotion, MotionConfig } from "motion/react";
import { ApiProvider } from "@/api/context";
import { createMockApi, type MockOptions } from "@/api/mock/adapter";
import type { PortalApi } from "@/api/portal-api";
import { AppearanceProvider } from "@/features/appearance";
import { StepUpProvider } from "@/features/step-up";
import { createAppRouter } from "@/router";
import { ToastProvider } from "@/ui/toast";
import { TooltipProvider } from "@/ui/tooltip";

export interface Rendered {
	api: PortalApi;
	router: ReturnType<typeof createAppRouter>;
}

/** The whole app on a memory history, against the mock with no latency. */
export async function renderRoute(
	path: string,
	options: MockOptions & { api?: PortalApi } = {},
): Promise<Rendered & ReturnType<typeof render>> {
	const { api: given, ...mock } = options;
	const api = given ?? createMockApi({ latency: 0, ...mock });
	const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
	const router = createAppRouter(
		{ queryClient, api },
		createMemoryHistory({ initialEntries: [path] }),
	);
	await router.load();
	const utils = render(
		<ApiProvider api={api}>
			<QueryClientProvider client={queryClient}>
				<AppearanceProvider>
					<CSPProvider disableStyleElements>
						<MotionConfig reducedMotion="always">
							<LazyMotion features={domAnimation} strict>
								<TooltipProvider>
									<ToastProvider>
										<StepUpProvider>
											<RouterProvider router={router} />
										</StepUpProvider>
									</ToastProvider>
								</TooltipProvider>
							</LazyMotion>
						</MotionConfig>
					</CSPProvider>
				</AppearanceProvider>
			</QueryClientProvider>
		</ApiProvider>,
	);
	return { ...utils, api, router };
}

export const heading = (name: string | RegExp, level = 1) =>
	screen.findByRole("heading", { name, level });
