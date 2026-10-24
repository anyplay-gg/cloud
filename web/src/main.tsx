import { CSPProvider } from "@base-ui/react/csp-provider";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider } from "@tanstack/react-router";
import { domAnimation, LazyMotion, MotionConfig } from "motion/react";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { createDefaultApi } from "@/api";
import { ApiProvider } from "@/api/context";
import { AppearanceProvider } from "@/features/appearance";
import { StepUpProvider } from "@/features/step-up";
import { ToastProvider } from "@/ui/toast";
import { TooltipProvider } from "@/ui/tooltip";
import { createAppRouter } from "./router";
import "./styles/index.css";

async function start() {
	const api = await createDefaultApi();
	const queryClient = new QueryClient({
		defaultOptions: { queries: { staleTime: 15_000, refetchOnWindowFocus: false } },
	});
	const router = createAppRouter({ queryClient, api });
	const root = document.getElementById("root");
	if (!root) throw new Error("#root is missing from index.html");
	createRoot(root).render(
		<StrictMode>
			<ApiProvider api={api}>
				<QueryClientProvider client={queryClient}>
					<AppearanceProvider>
						{/* The CSP has no 'unsafe-inline' for styles, so Base UI must not inject <style>. */}
						<CSPProvider disableStyleElements>
							{/* "user" turns movement into fades for people who asked for less of it. */}
							<MotionConfig reducedMotion="user">
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
			</ApiProvider>
		</StrictMode>,
	);
}

void start();
