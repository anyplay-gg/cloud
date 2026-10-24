import { createFileRoute } from "@tanstack/react-router";
import { DataPage } from "@/screens/data";

export const Route = createFileRoute("/account/data")({
	component: DataPage,
});
