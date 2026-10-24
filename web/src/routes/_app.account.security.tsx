import { createFileRoute } from "@tanstack/react-router";
import { Security } from "@/screens/security";

export const Route = createFileRoute("/_app/account/security")({
	component: Security,
});
