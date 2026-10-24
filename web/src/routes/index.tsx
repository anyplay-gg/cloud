import { createFileRoute, redirect } from "@tanstack/react-router";
import { qk } from "@/api/queries";
import { Landing } from "@/screens/landing";

// Signed in: straight to the overview. Signed out, or when the API cannot be reached at all
// (the landing may ship before the JSON API does): the landing page.
export const Route = createFileRoute("/")({
	beforeLoad: async ({ context }) => {
		let signedIn = false;
		try {
			const s = await context.queryClient.ensureQueryData({
				queryKey: qk.session,
				queryFn: () => context.api.session(),
				staleTime: 60_000,
			});
			signedIn = s.signedIn;
		} catch {
			signedIn = false;
		}
		if (signedIn) throw redirect({ to: "/account" });
	},
	component: Landing,
});
