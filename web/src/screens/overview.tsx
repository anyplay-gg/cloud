import { Link } from "@tanstack/react-router";
import { usePageTitle } from "@/lib/use-page-title";
import {
	ArrowUpRight,
	CircleCheck,
	KeyRound,
	LogIn,
	LogOut,
	type LucideIcon,
	PencilLine,
	ShieldAlert,
	TriangleAlert,
	UserRound,
} from "lucide-react";
import * as motion from "motion/react-m";
import {
	qk,
	useAction,
	useActivity,
	useDevices,
	useFriends,
	usePasskeys,
	useSession,
	useTrophies,
} from "@/api/queries";
import type { ActivityEvent, ActivityKind, Me } from "@/api/types";
import { Page, PageHeader, Section } from "@/components/layout";
import { ProfileBanner } from "@/components/profile-banner";
import { QueryView } from "@/components/query-state";
import { m } from "@/i18n";
import { cn } from "@/lib/cn";
import { daysUntil, formatDate, formatNumber, relativeTime } from "@/lib/format";
import { Avatar } from "@/ui/avatar";
import { Button, buttonClass } from "@/ui/button";
import { EmptyState, Skeleton } from "@/ui/feedback";

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
	const first = me.displayName.split(" ")[0] ?? me.displayName;
	return (
		<Page width="wide">
			<PageHeader title={m.overview_title({ name: first })} description={m.overview_lead()} />
			{me.deletion ? <DeletionNotice me={me} /> : null}
			<motion.div {...stage(0)}>
				<IdentityCard me={me} />
			</motion.div>
			<motion.div {...stage(1)}>
				<StatStrip />
			</motion.div>
			<div className="grid gap-8 lg:grid-cols-2">
				<motion.div {...stage(2)}>
					<SecuritySummary />
				</motion.div>
				<motion.div {...stage(3)}>
					<RecentActivity />
				</motion.div>
			</div>
		</Page>
	);
}

function DeletionNotice({ me }: { me: Me }) {
	const cancel = useAction((_: void, api) => api.data.cancelDeletion(), {
		success: m.toast_deletion_cancelled(),
		invalidate: [qk.session],
	});
	if (!me.deletion) return null;
	return (
		<div
			role="status"
			className="flex flex-col gap-4 rounded-xl bg-warning-soft p-5 shadow-[0_0_0_1px_var(--warning-7)] sm:flex-row sm:items-center sm:justify-between"
		>
			<div className="flex items-start gap-3">
				<TriangleAlert aria-hidden className="mt-0.5 size-5 shrink-0 text-warning-text" />
				<div>
					<p className="text-body font-semibold text-fg">
						{m.deletion_banner_title({ date: formatDate(me.deletion.scheduledFor) })}
					</p>
					<p className="mt-0.5 text-body text-fg-muted">
						{m.deletion_banner_body({ days: daysUntil(me.deletion.scheduledFor) })}
					</p>
				</div>
			</div>
			<Button variant="secondary" onClick={() => cancel.mutate()} loading={cancel.isPending}>
				{m.deletion_cancel()}
			</Button>
		</div>
	);
}

/** The profile as others see it, in small: banner, avatar, name, one line about you. */
function IdentityCard({ me }: { me: Me }) {
	return (
		<section
			aria-label={m.overview_profile_label()}
			className="overflow-hidden rounded-xl bg-surface shadow-card"
		>
			<ProfileBanner theme={me.theme} className="h-24 sm:h-28" />
			<div className="relative flex flex-col gap-4 px-5 pb-5 sm:flex-row sm:items-start sm:justify-between">
				<div className="flex min-w-0 items-start gap-4">
					<Avatar
						name={me.displayName}
						accent={me.accent}
						size="lg"
						className="-mt-8 rounded-full ring-4 ring-surface"
					/>
					<div className="min-w-0 pt-2.5">
						<p className="truncate text-lead font-semibold text-fg">{me.displayName}</p>
						<p className="truncate font-mono text-small text-fg-muted">@{me.handle}</p>
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
				{me.about ? <span className="text-fg">{me.about}</span> : m.overview_no_about()}
			</p>
		</section>
	);
}

function StatStrip() {
	const trophies = useTrophies();
	const friends = useFriends();
	const passkeys = usePasskeys();
	const devices = useDevices();
	return (
		<section aria-label={m.overview_stats_label()}>
			<ul className="grid grid-cols-2 gap-px overflow-hidden rounded-xl bg-line shadow-card lg:grid-cols-4">
				<Stat
					to="/account/trophies"
					label={m.nav_trophies()}
					loading={trophies.isPending}
					value={trophies.data ? `${trophies.data.percent}%` : undefined}
					note={
						trophies.data
							? m.overview_trophy_note({ count: trophies.data.titles.length })
							: undefined
					}
					error={trophies.isError}
				/>
				<Stat
					to="/account/friends"
					label={m.nav_friends()}
					loading={friends.isPending}
					value={friends.data ? formatNumber(friends.data.friends.length) : undefined}
					note={
						friends.data
							? friends.data.incoming.length > 0
								? m.overview_friends_requests({ count: friends.data.incoming.length })
								: m.overview_friends_none()
							: undefined
					}
					highlight={(friends.data?.incoming.length ?? 0) > 0}
					error={friends.isError}
				/>
				<Stat
					to="/account/security"
					label={m.overview_passkeys()}
					loading={passkeys.isPending}
					value={passkeys.data ? formatNumber(passkeys.data.length) : undefined}
					note={
						passkeys.data
							? passkeys.data.length === 0
								? m.overview_passkeys_none()
								: m.overview_passkeys_note()
							: undefined
					}
					error={passkeys.isError}
				/>
				<Stat
					to="/account/security"
					label={m.overview_devices()}
					loading={devices.isPending}
					value={devices.data ? formatNumber(devices.data.length) : undefined}
					note={devices.data ? m.overview_devices_note() : undefined}
					error={devices.isError}
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
}: {
	to: "/account/trophies" | "/account/friends" | "/account/security";
	label: string;
	value?: string;
	note?: string;
	loading?: boolean;
	error?: boolean;
	highlight?: boolean;
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
						<span className="nums text-display font-semibold text-fg">{value}</span>
						<span
							className={cn(
								"text-small",
								highlight ? "font-medium text-accent-text" : "text-fg-muted",
							)}
						>
							{note}
						</span>
					</>
				)}
			</Link>
		</li>
	);
}

function SecuritySummary() {
	const passkeys = usePasskeys();
	const devices = useDevices();
	const hasPasskey = (passkeys.data?.length ?? 0) > 0;
	const items: { ok: boolean | undefined; title: string; text: string }[] = [
		{
			ok: passkeys.isPending ? undefined : hasPasskey,
			title: hasPasskey ? m.overview_check_passkey_ok() : m.overview_check_passkey_todo(),
			text: hasPasskey ? m.overview_check_passkey_ok_text() : m.overview_check_passkey_todo_text(),
		},
		{
			ok: true,
			title: m.overview_check_email_ok(),
			text: m.overview_check_email_ok_text(),
		},
		{
			ok: devices.isPending ? undefined : true,
			title: m.overview_check_devices(),
			text: devices.data ? m.overview_check_devices_text({ count: devices.data.length }) : "",
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

const ACTIVITY: Record<ActivityKind, { icon: LucideIcon; label: () => string }> = {
	sign_in: { icon: LogIn, label: () => m.activity_sign_in() },
	passkey_added: { icon: KeyRound, label: () => m.activity_passkey_added() },
	passkey_removed: { icon: KeyRound, label: () => m.activity_passkey_removed() },
	device_signed_out: { icon: LogOut, label: () => m.activity_device_signed_out() },
	signed_out_everywhere: { icon: LogOut, label: () => m.activity_signed_out_everywhere() },
	profile_updated: { icon: UserRound, label: () => m.activity_profile_updated() },
};

function RecentActivity() {
	const activity = useActivity();
	return (
		<Section title={m.overview_activity()} description={m.overview_activity_lead()} flush>
			<QueryView
				query={activity}
				loadingLabel={m.overview_activity_loading()}
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
				isEmpty={(d) => d.length === 0}
				empty={
					<EmptyState
						title={m.overview_activity_empty()}
						description={m.overview_activity_empty_text()}
					/>
				}
			>
				{(events) => (
					<ul className="divide-y divide-line">
						{events.slice(0, 5).map((e) => (
							<ActivityRow key={e.id} event={e} />
						))}
					</ul>
				)}
			</QueryView>
		</Section>
	);
}

function ActivityRow({ event }: { event: ActivityEvent }) {
	const def = ACTIVITY[event.kind];
	const Icon = def.icon;
	return (
		<li className="flex items-center gap-3 px-5 py-3.5">
			<span className="flex size-8 shrink-0 items-center justify-center rounded-full bg-raised text-fg-muted shadow-[inset_0_0_0_1px_var(--line)]">
				<Icon aria-hidden strokeWidth={1.75} className="size-4" />
			</span>
			<div className="min-w-0 flex-1">
				<p className="truncate text-body font-medium text-fg">{def.label()}</p>
				{event.detail ? <p className="truncate text-small text-fg-muted">{event.detail}</p> : null}
			</div>
			<time dateTime={event.at} className="shrink-0 text-small text-fg-muted">
				{relativeTime(event.at)}
			</time>
		</li>
	);
}
