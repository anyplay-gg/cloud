import { createFileRoute } from "@tanstack/react-router";
import { ReturnScreen } from "@/screens/return";

// A static page: the launcher's own callback may send the browser here when it is done, after the
// cloud has ended the browser session, so there is nothing to check.
export const Route = createFileRoute("/return")({
	component: ReturnScreen,
});
