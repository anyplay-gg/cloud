import { createFileRoute, redirect } from "@tanstack/react-router";
import { qk } from "@/api/queries";
import { authSearch } from "@/lib/search";
import { AuthScreen } from "@/screens/auth";

export const Route = createFileRoute("/sign-in")({
	validateSearch: authSearch,
	beforeLoad: async ({ context, search }) => {
		const s = await context.queryClient.ensureQueryData({
			queryKey: qk.session,
			queryFn: () => context.api.session(),
			staleTime: 60_000,
		});
		if (s.signedIn && s.flow.step === "done") throw redirect({ to: search.next ?? "/account" });
	},
	component: SignIn,
});

function SignIn() {
	const { next } = Route.useSearch();
	return <AuthScreen mode="sign_in" next={next} />;
}
