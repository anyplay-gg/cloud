import { createFileRoute } from "@tanstack/react-router";
import { Privacy } from "@/screens/privacy";

export const Route = createFileRoute("/_app/account/privacy")({
	component: Privacy,
});
