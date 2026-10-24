import { Link } from "@tanstack/react-router";
import {
	ArrowUpRight,
	CircleCheck,
	Globe,
	type LucideIcon,
	Monitor,
	PencilLine,
	ShieldAlert,
	TriangleAlert,
} from "lucide-react";
import * as motion from "motion/react-m";
import {
	qk,
	useAccountView,
	useAction,
	useFriends,
	useOwnProfile,
	useSession,
	useTrophies,
} from "@/api/queries";
import type { Me } from "@/api/types";
import { Page, PageHeader, Section } from "@/components/layout";
import { ProfileBanner } from "@/components/profile-banner";
import { QueryView } from "@/components/query-state";
import { m } from "@/i18n";
import { daysUntil, formatDate, formatNumber, relativeTime } from "@/lib/format";
import { accentFor, avatarUrl, themeFor } from "@/lib/identity";
import { total } from "@/lib/trophies";
import { usePageTitle } from "@/lib/use-page-title";
import { Avatar, AvatarGroup } from "@/ui/avatar";
import { Button, buttonClass } from "@/ui/button";
import { Skeleton } from "@/ui/feedback";
import { RollingNumber } from "@/ui/rolling-number";

const EASE_OUT = [0.23, 1, 0.32, 1] as const;

// The overview is the first thing seen after sign-in, so it gets a short staged entrance: once
// per page load, 40ms between blocks, never again while moving around the portal.
let entered = false;
const stage = (i: number) =>
	entered
		? {}
		: {
				initial: { opacity: 0, y: 8 },
				animate: { opacity: 1, y: 0 },
				transition: { duration: 0.28, ease: EASE_OUT, delay: i * 0.04 },
				onAnimationComplete: () => {
					entered = true;
				},
			};

export function Overview() {
	usePageTitle(m.nav_overview());
	const me = useSession().data?.account;
	if (!me) return null;
	return (
		<Page width="wide">
			<PageHeader title={m.overview_title({ name: me.handle })} description={m.overview_lead()} />
			<DeletionNotice />
			<motion.div {...stage(0)}>
				<IdentityCard me={me} />
			</motion.div>
			<motion.div {...stage(1)}>
				<StatStrip />
			</motion.div>
			<div className="grid gap-10 lg:grid-cols-2">
				<motion.div {...stage(2)}>
					<SecuritySummary />
				</motion.div>
				<motion.div {...stage(3)}>
					<SignedIn />
				</motion.div>
			</div>
		</Page>
	);
}

function DeletionNotice() {
	const account = useAccountView();
	const cancel = useAction((_: undefined, api) => api.data.cancelDeletion(), {
		success: m.toast_deletion_cancelled(),
		invalidate: [qk.account],
	});
	const deletion = account.data?.deletion;
	if (!deletion) return null;
	return (
		<div
			role="status"
			className="flex flex-col gap-4 rounded-xl bg-warning-soft p-5 shadow-[0_0_0_1px_var(--warning-7)] sm:flex-row sm:items-center sm:justify-between"
		>
			<div className="flex items-start gap-3">
				<TriangleAlert aria-hidden className="mt-0.5 size-5 shrink-0 text-warning-text" />
				<div>
					<p className="text-body font-semibold text-fg">
						{m.deletion_banner_title({ date: formatDate(deletion.at) })}
					</p>
					<p className="mt-0.5 text-body text-fg-muted">
						{m.deletion_banner_body({ days: daysUntil(deletion.at) })}
					</p>
				</div>
			</div>
			<Button
				variant="secondary"
				onClick={() => cancel.mutate(undefined)}
				loading={cancel.isPending}
			>
				{m.deletion_cancel()}
			</Button>
		</div>
	);
}

/** The profile as others see it, in small: banner, avatar, name, one line about you. */
function IdentityCard({ me }: { me: Me }) {
	const profile = useOwnProfile();
	const about = profile.data?.about;
	return (
		<section
			aria-label={m.overview_profile_label()}
			className="overflow-hidden rounded-xl bg-surface shadow-card"
		>
			<ProfileBanner theme={themeFor(me.handle)} className="h-24 sm:h-28" />
			<div className="relative flex flex-col gap-4 px-5 pb-5 sm:flex-row sm:items-start sm:justify-between">
				<div className="flex min-w-0 items-start gap-4">
					<Avatar
						name={me.handle}
						accent={accentFor(me.handle)}
						src={avatarUrl(profile.data?.avatar)}
						size="lg"
						className="-mt-8 rounded-full ring-4 ring-surface"
					/>
					<div className="min-w-0 pt-2.5">
						<p className="truncate font-mono text-lead font-semibold text-fg">@{me.handle}</p>
						<p className="truncate text-small text-fg-muted">{me.email}</p>
					</div>
				</div>
				<div className="flex items-center gap-3 sm:pt-3">
					<Link to="/account/profile" className={buttonClass({ variant: "secondary" })}>
						<PencilLine aria-hidden />
						{m.overview_edit_profile()}
					</Link>
					<Link
						to="/u/$name"
						params={{ name: me.handle }}
						className={buttonClass({ variant: "ghost" })}
					>
						{m.menu_view_profile()}
						<ArrowUpRight aria-hidden />
					</Link>
				</div>
			</div>
			<p className="px-5 pb-5 text-body text-fg-muted">
				{profile.isPending ? (
					<Skeleton className="h-4 w-64 max-w-full" />
				) : about ? (
					<span className="text-fg">{about}</span>
				) : (
					m.overview_no_about()
				)}
			</p>
		</section>
	);
}

function StatStrip() {
	const trophies = useTrophies();
	const friends = useFriends();
	const account = useAccountView();
	const online = friends.data?.friends.filter((f) => f.presence.online) ?? [];
	return (
		<section aria-label={m.overview_stats_label()}>
			<ul className="grid grid-cols-2 gap-px overflow-hidden rounded-xl bg-line shadow-card lg:grid-cols-4">
				<Stat
					to="/trophies"
					label={m.nav_trophies()}
					loading={trophies.isPending}
					error={trophies.isError}
					value={trophies.data ? total(trophies.data.counts) : undefined}
					rolling
					note={
						trophies.data
							? m.overview_trophy_note({ count: trophies.data.titles.length })
							: undefined
					}
				/>
				<Stat
					to="/friends"
					label={m.nav_friends()}
					loading={friends.isPending}
					error={friends.isError}
					value={friends.data?.friends.length}
					highlight={(friends.data?.incoming.length ?? 0) > 0}
					note={
						friends.data
							? friends.data.incoming.length > 0
								? m.overview_friends_requests({ count: friends.data.incoming.length })
								: m.overview_friends_online({ count: online.length })
							: undefined
					}
					extra={
						online.length > 0 ? (
							<AvatarGroup
								people={online.map((f) => ({
									name: f.account.handle,
									accent: accentFor(f.account.handle),
								}))}
								label={m.friends_online_group({ count: online.length })}
								size="xs"
								max={3}
							/>
						) : undefined
					}
				/>
				<Stat
					to="/account/security"
					label={m.overview_passkeys()}
					loading={account.isPending}
					error={account.isError}
					value={account.data?.passkeys.length}
					note={
						account.data
							? account.data.passkeys.length === 0
								? m.overview_passkeys_none()
								: m.overview_passkeys_note()
							: undefined
					}
				/>
				<Stat
					to="/account/security"
					label={m.overview_devices()}
					loading={account.isPending}
					error={account.isError}
					value={
						account.data ? account.data.devices.length + account.data.sessions.length : undefined
					}
					note={account.data ? m.overview_devices_note() : undefined}
				/>
			</ul>
		</section>
	);
}

function Stat({
	to,
	label,
	value,
	note,
	loading,
	error,
	highlight,
	rolling,
	extra,
}: {
	to: "/trophies" | "/friends" | "/account/security";
	label: string;
	value?: number;
	note?: string;
	loading?: boolean;
	error?: boolean;
	highlight?: boolean;
	rolling?: boolean;
	extra?: React.ReactNode;
}) {
	return (
		<li className="bg-surface">
			<Link
				to={to}
				className="group press flex h-full flex-col gap-1 p-5 outline-offset-[-2px] pointer-hover:hover:bg-hover"
			>
				<span className="flex items-center justify-between text-small font-medium text-fg-muted">
					{label}
					<ArrowUpRight
						aria-hidden
						className="size-3.5 opacity-0 transition-opacity duration-[var(--dur-quick)] group-focus-visible:opacity-100 pointer-hover:group-hover:opacity-100"
					/>
				</span>
				{loading ? (
					<>
						<Skeleton className="mt-1 h-8 w-16" />
						<Skeleton className="mt-1 h-4 w-24" />
						<span className="sr-only">{m.common_loading()}</span>
					</>
				) : error ? (
					<>
						<span className="text-display font-semibold text-fg-faint">–</span>
						<span className="text-small text-danger-text">{m.overview_stat_error()}</span>
					</>
				) : (
					<>
						<span className="text-display font-semibold text-fg">
							{rolling && value !== undefined ? (
								<RollingNumber value={value} />
							) : (
								<span className="nums">{value === undefined ? "" : formatNumber(value)}</span>
							)}
						</span>
						<span className="flex items-center gap-2">
							{extra}
							<span
								className={
									highlight ? "text-small font-medium text-accent-text" : "text-small text-fg-muted"
								}
							>
								{note}
							</span>
						</span>
					</>
				)}
			</Link>
		</li>
	);
}

function SecuritySummary() {
	const account = useAccountView();
	const hasPasskey = (account.data?.passkeys.length ?? 0) > 0;
	const pending = account.isPending;
	const items: { ok: boolean | undefined; title: string; text: string }[] = [
		{
			ok: pending ? undefined : hasPasskey,
			title: hasPasskey ? m.overview_check_passkey_ok() : m.overview_check_passkey_todo(),
			text: hasPasskey ? m.overview_check_passkey_ok_text() : m.overview_check_passkey_todo_text(),
		},
		{ ok: true, title: m.overview_check_email_ok(), text: m.overview_check_email_ok_text() },
		{
			ok: pending ? undefined : true,
			title: m.overview_check_devices(),
			text: account.data
				? m.overview_check_devices_text({
						count: account.data.devices.length + account.data.sessions.length,
					})
				: "",
		},
	];
	return (
		<Section
			title={m.nav_security()}
			description={m.overview_security_lead()}
			flush
			actions={
				<Link to="/account/security" className={buttonClass({ variant: "ghost", size: "sm" })}>
					{m.overview_manage()}
				</Link>
			}
		>
			<ul className="divide-y divide-line">
				{items.map((it) => (
					<li key={it.title} className="flex items-start gap-3 px-5 py-4">
						{it.ok === undefined ? (
							<Skeleton className="mt-0.5 size-5 rounded-full" />
						) : it.ok ? (
							<CircleCheck aria-hidden className="mt-0.5 size-5 shrink-0 text-success-text" />
						) : (
							<ShieldAlert aria-hidden className="mt-0.5 size-5 shrink-0 text-warning-text" />
						)}
						<div className="min-w-0">
							<p className="text-body font-medium text-fg">{it.title}</p>
							<p className="mt-0.5 text-body text-fg-muted">{it.text}</p>
						</div>
					</li>
				))}
			</ul>
		</Section>
	);
}

/** The most recently used launchers and browsers, so a stranger among them is easy to spot. */
function SignedIn() {
	const account = useAccountView();
	return (
		<Section
			title={m.devices_title()}
			description={m.overview_signed_in_lead()}
			flush
			actions={
				<Link to="/account/security" className={buttonClass({ variant: "ghost", size: "sm" })}>
					{m.overview_manage()}
				</Link>
			}
		>
			<QueryView
				query={account}
				loadingLabel={m.devices_loading()}
				loading={
					<ul className="divide-y divide-line">
						{[0, 1, 2].map((i) => (
							<li key={i} className="flex items-center gap-3 px-5 py-4">
								<Skeleton className="size-8 rounded-full" />
								<div className="flex-1 space-y-2">
									<Skeleton className="h-4 w-1/3" />
									<Skeleton className="h-3 w-1/2" />
								</div>
							</li>
						))}
					</ul>
				}
				errorTitle={m.devices_error()}
			>
				{(a) => {
					const rows: {
						key: string;
						icon: LucideIcon;
						title: string;
						detail: string;
						at: number;
						current: boolean;
					}[] = [
						...a.devices.map((d) => ({
							key: `d${d.id}`,
							icon: Monitor,
							title: d.name,
							detail: m.devices_launcher(),
							at: d.lastUsed,
							current: false,
						})),
						...a.sessions.map((s) => ({
							key: `s${s.id}`,
							icon: Globe,
							title: s.current ? m.devices_this_browser() : m.devices_browser(),
							detail: m.devices_signed_in_since({ date: formatDate(s.created, "medium") }),
							at: s.lastUsed,
							current: s.current,
						})),
					]
						.sort((x, y) => y.at - x.at)
						.slice(0, 4);
					return (
						<ul className="divide-y divide-line">
							{rows.map((r) => (
								<li key={r.key} className="flex items-center gap-3 px-5 py-3.5">
									<span className="flex size-8 shrink-0 items-center justify-center rounded-full bg-raised text-fg-muted shadow-[inset_0_0_0_1px_var(--line)]">
										<r.icon aria-hidden strokeWidth={1.75} className="size-4" />
									</span>
									<div className="min-w-0 flex-1">
										<p className="truncate text-body font-medium text-fg">{r.title}</p>
										<p className="truncate text-small text-fg-muted">{r.detail}</p>
									</div>
									<time
										dateTime={new Date(r.at * 1000).toISOString()}
										className="shrink-0 text-small text-fg-muted"
									>
										{r.current ? m.devices_active_now() : relativeTime(r.at)}
									</time>
								</li>
							))}
						</ul>
					);
				}}
			</QueryView>
		</Section>
	);
}
