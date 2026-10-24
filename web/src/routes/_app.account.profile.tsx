import { createFileRoute } from "@tanstack/react-router";
import { ProfileEdit } from "@/screens/profile";

export const Route = createFileRoute("/_app/account/profile")({
	component: ProfileEdit,
});
