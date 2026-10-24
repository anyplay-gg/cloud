import { Link } from "@tanstack/react-router";
import type { ReactNode } from "react";
import { m } from "@/i18n";
import { AppearanceMenu } from "./app-shell";
import { Wordmark } from "./logo";

/**
 * One centred column for sign-in, account creation and the hand-off. A faint glow of the accent
 * sits behind the top of the page, the way the launcher's backdrop lights its upper right.
 */
export function AuthLayout({ children, footer }: { children: ReactNode; footer?: ReactNode }) {
	return (
		<div className="relative isolate flex min-h-dvh flex-col">
			<div
				aria-hidden
				className="pointer-events-none absolute inset-x-0 top-0 -z-10 h-[28rem] bg-[radial-gradient(60%_100%_at_50%_0%,color-mix(in_oklab,var(--accent)_13%,transparent),transparent)]"
			/>
			<header className="flex h-16 items-center justify-between px-5 sm:px-8">
				<Link to="/" aria-label={m.nav_home_label()} className="rounded-md outline-offset-4">
					<Wordmark />
				</Link>
				<AppearanceMenu />
			</header>
			<main
				id="main"
				className="flex flex-1 flex-col items-center px-5 pb-16 pt-8 sm:pt-[min(12vh,6rem)]"
			>
				<div className="w-full max-w-[24rem]">
					{children}
					{footer ? <div className="mt-8 border-t border-line pt-6">{footer}</div> : null}
				</div>
			</main>
		</div>
	);
}
