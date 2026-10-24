import { Link } from "@tanstack/react-router";
import { usePageTitle } from "@/lib/use-page-title";
import { Check } from "lucide-react";
import * as motion from "motion/react-m";
import { useSession } from "@/api/queries";
import { AuthLayout } from "@/components/auth-layout";
import { m } from "@/i18n";
import { buttonClass } from "@/ui/button";

/** Where the browser lands after the launcher asked for a sign-in. Nothing to do but go back. */
export function ReturnScreen() {
	usePageTitle(m.return_title());
	const me = useSession().data?.account;
	return (
		<AuthLayout>
			<div className="flex flex-col items-start gap-6">
				<motion.div
					initial={{ opacity: 0, scale: 0.85 }}
					animate={{ opacity: 1, scale: 1 }}
					transition={{ type: "spring", duration: 0.45, bounce: 0 }}
					className="flex size-14 items-center justify-center rounded-full bg-accent text-on-accent shadow-[0_0_0_6px_color-mix(in_oklab,var(--accent)_16%,transparent),0_0_28px_color-mix(in_oklab,var(--accent)_50%,transparent)]"
				>
					<Check aria-hidden strokeWidth={2.5} className="size-7" />
				</motion.div>
				<div>
					<h1 className="text-heading font-semibold">{m.return_title()}</h1>
					<p className="mt-2 text-body text-fg-muted">
						{me ? m.return_lead_named({ handle: me.handle }) : m.return_lead()}
					</p>
				</div>
				<Link to="/account" className={buttonClass({ variant: "secondary", size: "lg" })}>
					{m.return_account()}
				</Link>
			</div>
		</AuthLayout>
	);
}
