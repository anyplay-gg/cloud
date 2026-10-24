import { Link } from "@tanstack/react-router";
import type { LucideIcon } from "lucide-react";
import {
	ArrowUpRight,
	BookOpen,
	Download,
	GitBranch,
	MessageCircle,
	Mic,
	Trophy,
	Users,
} from "lucide-react";
import { AppearanceMenu } from "@/components/app-shell";
import { LauncherMockup } from "@/components/launcher-mockup";
import { Wordmark } from "@/components/logo";
import { MagneticDock } from "@/components/magnetic-dock";
import { m } from "@/i18n";
import { usePageTitle } from "@/lib/use-page-title";
import { buttonClass } from "@/ui/button";

const RELEASES = "https://github.com/anyplay-gg/launcher/releases/latest";
const RELEASES_ALL = "https://github.com/anyplay-gg/launcher/releases";
const GITHUB = "https://github.com/anyplay-gg";
const DISCORD = "https://discord.gg/nSHbHBRdS";

const ext = { target: "_blank", rel: "noopener noreferrer" } as const;
const tab = <span className="sr-only"> {m.common_new_tab()}</span>;

/** The signed-out front page. Markup and CSS only: it needs no API, so it can ship on its own. */
export function Landing() {
	usePageTitle("");
	const features: { icon: LucideIcon; title: string; text: string }[] = [
		{ icon: Users, title: m.landing_f1_title(), text: m.landing_f1_text() },
		{ icon: Mic, title: m.landing_f2_title(), text: m.landing_f2_text() },
		{ icon: Trophy, title: m.landing_f3_title(), text: m.landing_f3_text() },
		{ icon: GitBranch, title: m.landing_f4_title(), text: m.landing_f4_text() },
	];
	return (
		<div className="min-h-dvh">
			<div
				data-profile-theme="cosmic"
				className="relative isolate overflow-hidden bg-[linear-gradient(180deg,var(--t-mid),var(--t-deep)_78%)] text-fg [color-scheme:dark]"
			>
				<div aria-hidden className="landing-glow landing-glow-a" />
				<div aria-hidden className="landing-glow landing-glow-b" />
				<header className="mx-auto flex h-16 max-w-[68rem] items-center justify-between px-5 sm:px-8">
					<Link to="/" aria-label={m.nav_home_label()} className="rounded-md outline-offset-4">
						<Wordmark />
					</Link>
					<nav aria-label={m.nav_main_label()} className="flex items-center gap-1.5">
						<a
							href={GITHUB}
							{...ext}
							className={buttonClass({ variant: "ghost", className: "max-sm:hidden" })}
						>
							GitHub
							{tab}
						</a>
						<a
							href={DISCORD}
							{...ext}
							className={buttonClass({ variant: "ghost", className: "max-sm:hidden" })}
						>
							Discord
							{tab}
						</a>
						<Link to="/sign-in" search={{}} className={buttonClass({ variant: "ghost" })}>
							{m.auth_signin_link()}
						</Link>
					</nav>
				</header>
				<main
					id="main"
					className="mx-auto grid max-w-[68rem] items-center gap-14 px-5 pb-20 pt-14 sm:px-8 sm:pb-28 sm:pt-20 lg:grid-cols-[1.02fr_1fr]"
				>
					<div>
						<h1 className="max-w-[16ch] text-[clamp(2.5rem,6.2vw,4.25rem)] font-semibold leading-[1.02] tracking-[-0.032em] text-balance">
							{m.landing_title_pre()}
							<span className="marker">{m.landing_title_mark()}</span>
							{m.landing_title_post()}
						</h1>
						<p className="mt-6 max-w-[52ch] text-[clamp(1rem,1.6vw,1.1875rem)] leading-[1.55] text-fg-muted">
							{m.landing_lead()}
						</p>
						<div className="mt-9 flex flex-wrap items-center gap-3">
							<a
								href={RELEASES}
								{...ext}
								className={buttonClass({ variant: "primary", size: "lg" })}
							>
								<Download aria-hidden />
								{m.landing_download()}
								{tab}
							</a>
							<Link
								to="/create"
								search={{}}
								className={buttonClass({ variant: "secondary", size: "lg" })}
							>
								{m.auth_create_button()}
							</Link>
						</div>
						<p className="mt-4 text-small text-fg-muted">{m.landing_platforms()}</p>
						<div className="mt-10">
							<MagneticDock
								label={m.landing_dock_label()}
								links={[
									{ label: "GitHub", href: GITHUB, icon: GitBranch, external: true },
									{ label: "Discord", href: DISCORD, icon: MessageCircle, external: true },
									{
										label: m.landing_releases(),
										href: RELEASES_ALL,
										icon: Download,
										external: true,
									},
									{
										label: m.landing_docs(),
										href: "https://github.com/anyplay-gg/network/tree/main/docs",
										icon: BookOpen,
										external: true,
									},
								]}
							/>
						</div>
					</div>
					<div className="lg:translate-x-6">
						<LauncherMockup />
					</div>
				</main>
			</div>

			<section
				aria-labelledby="landing-features"
				className="mx-auto max-w-[68rem] px-5 py-20 sm:px-8 sm:py-28"
			>
				<h2
					id="landing-features"
					className="max-w-[24ch] text-title font-semibold tracking-[-0.02em] sm:text-display"
				>
					{m.landing_features_title()}
				</h2>
				<ul className="mt-10 grid gap-px overflow-hidden rounded-xl bg-line shadow-card sm:grid-cols-2">
					{features.map((f) => (
						<li key={f.title} className="flex flex-col gap-3 bg-surface p-6 sm:p-7">
							<span className="flex size-10 items-center justify-center rounded-lg bg-raised text-fg-muted shadow-[inset_0_0_0_1px_var(--line-strong)]">
								<f.icon aria-hidden strokeWidth={1.75} className="size-5" />
							</span>
							<h3 className="text-lead font-semibold">{f.title}</h3>
							<p className="max-w-[44ch] text-body text-fg-muted">{f.text}</p>
						</li>
					))}
				</ul>
			</section>

			<section aria-labelledby="landing-open" className="border-t border-line">
				<div className="mx-auto flex max-w-[68rem] flex-col gap-6 px-5 py-16 sm:flex-row sm:items-center sm:justify-between sm:px-8 sm:py-20">
					<div>
						<h2 id="landing-open" className="text-title font-semibold tracking-[-0.02em]">
							{m.landing_open_title()}
						</h2>
						<p className="mt-2 max-w-[56ch] text-body text-fg-muted">{m.landing_open_text()}</p>
					</div>
					<a
						href={GITHUB}
						{...ext}
						className={buttonClass({ variant: "secondary", size: "lg", className: "shrink-0" })}
					>
						{m.landing_open_cta()}
						<ArrowUpRight aria-hidden />
						{tab}
					</a>
				</div>
			</section>

			<footer className="border-t border-line">
				<div className="mx-auto flex max-w-[68rem] flex-col gap-4 px-5 py-8 text-small text-fg-muted sm:flex-row sm:items-center sm:justify-between sm:px-8">
					<p>{m.landing_footer()}</p>
					<ul className="flex flex-wrap items-center gap-x-6 gap-y-2">
						<li>
							<a href={GITHUB} {...ext} className="hover:text-fg">
								GitHub
								{tab}
							</a>
						</li>
						<li>
							<a href={DISCORD} {...ext} className="hover:text-fg">
								Discord
								{tab}
							</a>
						</li>
						<li>
							<a href={RELEASES_ALL} {...ext} className="hover:text-fg">
								{m.landing_releases()}
								{tab}
							</a>
						</li>
						<li>
							<Link to="/sign-in" search={{}} className="hover:text-fg">
								{m.auth_signin_link()}
							</Link>
						</li>
						<li>
							<AppearanceMenu side="top" />
						</li>
					</ul>
				</div>
			</footer>
		</div>
	);
}
