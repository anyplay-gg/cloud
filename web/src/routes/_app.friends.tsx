import { createFileRoute } from "@tanstack/react-router";
import { Friends } from "@/screens/friends";

export const Route = createFileRoute("/_app/friends")({
	component: Friends,
});
