import { createFileRoute } from "@tanstack/react-router";
import { ProfileEdit } from "@/screens/profile";

export const Route = createFileRoute("/account/profile")({
	component: ProfileEdit,
});
