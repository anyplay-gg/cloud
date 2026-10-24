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
		if (s.signedIn)
			throw redirect({ to: search.handoff ? "/return" : (search.next ?? "/account") });
	},
	component: Create,
});

function Create() {
	const { next, handoff } = Route.useSearch();
	return <AuthScreen mode="create" next={next} handoff={handoff} />;
}
