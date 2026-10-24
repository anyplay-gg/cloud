import { createFileRoute } from "@tanstack/react-router";
import { Security } from "@/screens/security";

export const Route = createFileRoute("/account/security")({
	component: Security,
});
