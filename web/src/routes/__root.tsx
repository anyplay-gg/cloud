import { createRootRouteWithContext, Link, Outlet, useRouterState } from "@tanstack/react-router";
import { useEffect, useRef, useState } from "react";
import { LogoMark } from "@/components/logo";
import { m } from "@/i18n";
import { buttonClass } from "@/ui/button";
import type { RouterContext } from "../router";

export const Route = createRootRouteWithContext<RouterContext>()({
	component: RootLayout,
	notFoundComponent: NotFound,
	errorComponent: ({ error, reset }) => (
		<CenteredMessage
			title={m.error_page_title()}
			body={error instanceof Error && import.meta.env.DEV ? error.message : m.error_page_body()}
		>
			<button type="button" className={buttonClass({ variant: "secondary" })} onClick={reset}>
				{m.common_try_again()}
			</button>
		</CenteredMessage>
	),
});

function NotFound() {
	return (
		<CenteredMessage title={m.not_found_title()} body={m.not_found_body()}>
			<Link to="/account" className={buttonClass({ variant: "secondary" })}>
				{m.not_found_action()}
			</Link>
		</CenteredMessage>
	);
}

function CenteredMessage({
	title,
	body,
	children,
}: {
	title: string;
	body: string;
	children?: React.ReactNode;
}) {
	return (
		<main className="mx-auto flex min-h-dvh max-w-sm flex-col items-start justify-center gap-5 px-6">
			<LogoMark />
			<div>
				<h1 className="text-heading font-semibold">{title}</h1>
				<p className="mt-2 text-body text-fg-muted">{body}</p>
			</div>
			{children}
		</main>
	);
}

function RootLayout() {
	return (
		<>
			<Outlet />
			<RouteAnnouncer />
		</>
	);
}

/**
 * A single-page app changes screens without a page load, so nothing tells a screen reader that
 * it happened. After each navigation: say the new page title, and put focus on the main region
 * so the next Tab starts at the content.
 */
function RouteAnnouncer() {
	const path = useRouterState({ select: (s) => s.location.pathname });
	const [message, setMessage] = useState("");
	const first = useRef(true);
	// biome-ignore lint/correctness/useExhaustiveDependencies: runs when the path changes
	useEffect(() => {
		const id = window.setTimeout(() => {
			setMessage(document.title);
			if (!first.current) document.getElementById("main")?.focus({ preventScroll: true });
			first.current = false;
		}, 120);
		return () => window.clearTimeout(id);
	}, [path]);
	return (
		<div role="status" aria-live="polite" className="sr-only">
			{message}
		</div>
	);
}
