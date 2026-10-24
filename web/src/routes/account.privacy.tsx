import { createFileRoute } from "@tanstack/react-router";
import { Privacy } from "@/screens/privacy";

export const Route = createFileRoute("/account/privacy")({
	component: Privacy,
});
