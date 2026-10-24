import { createFileRoute, Outlet, redirect } from "@tanstack/react-router";
import { qk } from "@/api/queries";
import { AppShell } from "@/components/app-shell";

export const Route = createFileRoute("/_app")({
	beforeLoad: async ({ context, location }) => {
		const s = await context.queryClient.ensureQueryData({
			queryKey: qk.session,
			queryFn: () => context.api.session(),
			staleTime: 60_000,
		});
		if (!s.signedIn) throw redirect({ to: "/sign-in", search: { next: location.href } });
		if (s.flow.step === "banned") throw redirect({ to: "/sign-in", search: {} });
	},
	component: () => (
		<AppShell>
			<Outlet />
		</AppShell>
	),
});
