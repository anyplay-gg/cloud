import { createFileRoute } from "@tanstack/react-router";
import { Banned } from "@/screens/banned";

export const Route = createFileRoute("/banned")({
	component: Banned,
});
