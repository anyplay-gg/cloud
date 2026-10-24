import { createFileRoute, redirect } from "@tanstack/react-router";
import { qk } from "@/api/queries";
import { authSearch } from "@/lib/search";
import { AuthScreen } from "@/screens/auth";

export const Route = createFileRoute("/create")({
	validateSearch: authSearch,
	beforeLoad: async ({ context, search }) => {
		const s = await context.queryClient.ensureQueryData({
			queryKey: qk.session,
			queryFn: () => context.api.session(),
			staleTime: 60_000,
		});
		if (s.signedIn && s.flow.step === "banned") throw redirect({ to: "/banned" });
		if (s.signedIn && s.flow.step === "done") throw redirect({ to: search.next ?? "/account" });
	},
	component: Create,
});

function Create() {
	const { next } = Route.useSearch();
	return <AuthScreen mode="create" next={next} />;
}
