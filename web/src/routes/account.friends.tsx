import { createFileRoute } from "@tanstack/react-router";
import { Friends } from "@/screens/friends";

export const Route = createFileRoute("/account/friends")({
	component: Friends,
});
