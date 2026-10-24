import { createFileRoute } from "@tanstack/react-router";
import { PublicProfileScreen } from "@/screens/public-profile";

export const Route = createFileRoute("/u/$name")({
	component: () => {
		const { name } = Route.useParams();
		return <PublicProfileScreen name={name} />;
	},
});
