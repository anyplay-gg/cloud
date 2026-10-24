import { createFileRoute } from "@tanstack/react-router";
import { Overview } from "@/screens/overview";

export const Route = createFileRoute("/account/")({
	component: Overview,
});
