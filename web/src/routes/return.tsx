import { createFileRoute, redirect } from "@tanstack/react-router";
import { qk } from "@/api/queries";
import { ReturnScreen } from "@/screens/return";

export const Route = createFileRoute("/return")({
	beforeLoad: async ({ context }) => {
		const s = await context.queryClient.ensureQueryData({
			queryKey: qk.session,
			queryFn: () => context.api.session(),
			staleTime: 60_000,
		});
		if (!s.signedIn) throw redirect({ to: "/sign-in", search: { handoff: true } });
	},
	component: ReturnScreen,
});
