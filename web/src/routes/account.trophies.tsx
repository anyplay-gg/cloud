import { createFileRoute } from "@tanstack/react-router";
import { Trophies } from "@/screens/trophies";

export const Route = createFileRoute("/account/trophies")({
	component: Trophies,
});
