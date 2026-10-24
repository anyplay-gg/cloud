import { Link } from "@tanstack/react-router";
import { Check, Lock, PencilLine, UserMinus, UserPlus, UserRoundX } from "lucide-react";
import { useState } from "react";
import { describeError, isApiError } from "@/api/errors";
import { qk, useAction, usePublicProfile, useSession } from "@/api/queries";
import type { Audience, ProfileField, PublicProfile } from "@/api/types";
import { AppearanceMenu } from "@/components/app-shell";
import { Wordmark } from "@/components/logo";
import { ProfileBanner } from "@/components/profile-banner";
import { GradeTiles } from "@/components/trophy-parts";
import { m } from "@/i18n";
import { formatMonthYear, formatNumber } from "@/lib/format";
import { usePageTitle } from "@/lib/use-page-title";
import { Avatar, PresenceDot } from "@/ui/avatar";
import { Button, buttonClass } from "@/ui/button";
import { ConfirmDialog } from "@/ui/dialog";
import { ErrorState, LoadingRegion, ProgressBar, Skeleton } from "@/ui/feedback";
import { Menu, MenuContent, MenuItem, MenuTrigger } from "@/ui/menu";

export function PublicProfileScreen({ name }: { name: string }) {
	const profile = usePublicProfile(name);
	const session = useSession();
	const signedIn = session.data?.signedIn ?? false;
	usePageTitle(profile.data ? `${profile.data.displayName} (@${profile.data.handle})` : `@${name}`);

	return (
		<div className="min-h-dvh">
			<header className="mx-auto flex h-16 max-w-[44rem] items-center justify-between px-4 sm:px-0">
				<Link to="/" aria-label={m.nav_home_label()} className="rounded-md outline-offset-4">
					<Wordmark />
				</Link>
				<div className="flex items-center gap-1.5">
					<AppearanceMenu />
					{session.isPending ? null : signedIn ? (
						<Link to="/account" className={buttonClass({ variant: "secondary" })}>
							{m.public_open_account()}
						</Link>
					) : (
						<>
							<Link
								to="/sign-in"
								search={{ next: `/u/${name}` }}
								className={buttonClass({ variant: "ghost" })}
							>
								{m.auth_signin_link()}
							</Link>
							<Link
								to="/create"
								search={{ next: `/u/${name}` }}
								className={buttonClass({ variant: "secondary", className: "max-sm:hidden" })}
							>
								{m.auth_create_button()}
							</Link>
						</>
					)}
				</div>
			</header>
			<main id="main" className="mx-auto max-w-[44rem] px-4 pb-20 pt-2 sm:px-0">
				{profile.isPending ? (
					<ProfileSkeleton />
				) : profile.isError ? (
					isApiError(profile.error) && profile.error.code === "not_found" ? (
						<NotFound name={name} />
					) : (
						<div className="rounded-xl bg-surface shadow-card">
							<ErrorState
								title={m.public_error_title()}
								description={describeError(profile.error)}
								onRetry={() => void profile.refetch()}
								retrying={profile.isFetching}
							/>
						</div>
					)
				) : (
					<ProfileView profile={profile.data} />
				)}
			</main>
		</div>
	);
}

function ProfileSkeleton() {
	return (
		<LoadingRegion label={m.public_loading()}>
			<div className="overflow-hidden rounded-2xl bg-surface shadow-card">
				<Skeleton className="h-40 rounded-none sm:h-48" />
				<div className="px-5 pb-6">
					<Skeleton className="-mt-12 size-24 rounded-full ring-4 ring-surface" />
					<Skeleton className="mt-4 h-8 w-56" />
					<Skeleton className="mt-2 h-4 w-28" />
					<Skeleton className="mt-5 h-4 w-80 max-w-full" />
				</div>
			</div>
		</LoadingRegion>
	);
}

function NotFound({ name }: { name: string }) {
	return (
		<div className="flex flex-col items-start gap-4 rounded-2xl bg-surface p-6 shadow-card sm:p-8">
			<span className="flex size-11 items-center justify-center rounded-lg bg-raised text-fg-muted shadow-[inset_0_0_0_1px_var(--line-strong)]">
				<UserRoundX aria-hidden strokeWidth={1.75} className="size-5" />
			</span>
			<div>
				<h1 className="text-title font-semibold">{m.public_not_found_title({ name })}</h1>
				<p className="mt-1.5 max-w-[48ch] text-body text-fg-muted">{m.public_not_found_text()}</p>
			</div>
		</div>
	);
}

const AUDIENCE_TEXT: Record<Exclude<Audience, "everyone">, (what: string, name: string) => string> = {
	friends: (what, name) => m.public_hidden_friends({ what, name }),
	me: (what, name) => m.public_hidden_me({ what, name }),
};
const FIELD_NAME: Record<ProfileField, () => string> = {
	about: () => m.public_field_about(),
	friends: () => m.public_field_friends(),
	trophies: () => m.public_field_trophies(),
	activity: () => m.public_field_activity(),
};

function ProfileView({ profile }: { profile: PublicProfile }) {
	const first = profile.displayName.split(" ")[0] ?? profile.displayName;
	const status = profile.status;
	return (
		<article className="flex flex-col gap-6" aria-label={profile.displayName}>
			<div className="overflow-hidden rounded-2xl bg-surface shadow-card">
				<ProfileBanner theme={profile.theme} className="h-40 sm:h-48" />
				<div className="px-5 pb-6 sm:px-6">
					<div className="flex items-start justify-between gap-4">
						<Avatar
							name={profile.displayName}
							accent={profile.accent}
							size="xl"
							className="-mt-12 rounded-full ring-4 ring-surface"
						/>
						<div className="pt-3">
							<FriendAction profile={profile} />
						</div>
					</div>
					<h1 className="mt-4 text-display font-semibold text-fg">{profile.displayName}</h1>
					<p className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-body text-fg-muted">
						<span className="font-mono">@{profile.handle}</span>
						{status ? (
							<span className="flex items-center gap-1.5">
								<PresenceDot status={status} />
								{status === "online"
									? m.status_online()
									: status === "away"
										? m.status_away()
										: m.status_offline()}
							</span>
						) : null}
						<span>{m.public_joined({ date: formatMonthYear(profile.joinedAt) })}</span>
					</p>
					{profile.visible && profile.about ? (
						<p className="mt-4 max-w-[56ch] text-prose text-fg">{profile.about}</p>
					) : null}
				</div>
			</div>

			{!profile.visible ? (
				<div className="flex items-start gap-4 rounded-xl bg-surface p-5 shadow-card">
					<Lock aria-hidden strokeWidth={1.75} className="mt-0.5 size-5 shrink-0 text-fg-muted" />
					<div>
						<h2 className="text-body font-semibold text-fg">{m.public_private_title()}</h2>
						<p className="mt-1 max-w-[52ch] text-body text-fg-muted">
							{profile.relation === "signed_out"
								? m.public_private_signed_out({ name: first })
								: m.public_private_text({ name: first })}
						</p>
					</div>
				</div>
			) : (
				<>
					{profile.friendCount !== null || profile.trophies ? (
						<ul className="grid grid-cols-2 gap-px overflow-hidden rounded-xl bg-line shadow-card">
							{profile.friendCount !== null ? (
								<li className="bg-surface p-5">
									<p className="text-small font-medium text-fg-muted">{m.nav_friends()}</p>
									<p className="nums mt-1 text-display font-semibold">{formatNumber(profile.friendCount)}</p>
								</li>
							) : null}
							{profile.trophies ? (
								<li className="bg-surface p-5">
									<p className="text-small font-medium text-fg-muted">{m.nav_trophies()}</p>
									<p className="nums mt-1 text-display font-semibold">{profile.trophies.percent}%</p>
								</li>
							) : null}
						</ul>
					) : null}
					{profile.trophies ? (
						<section aria-labelledby="pp-trophies" className="flex flex-col gap-3">
							<h2 id="pp-trophies" className="text-prose font-semibold">
								{m.public_trophies_title()}
							</h2>
							<div className="flex flex-col gap-5 rounded-xl bg-surface p-5 shadow-card">
								<ProgressBar
									value={profile.trophies.percent}
									label={m.public_trophies_progress({ name: first })}
								/>
								<GradeTiles earned={profile.trophies.earned} total={profile.trophies.total} />
							</div>
						</section>
					) : null}
					{profile.hidden.length > 0 ? (
						<ul className="flex flex-col gap-2">
							{profile.hidden
								.filter((h): h is { field: ProfileField; audience: Exclude<Audience, "everyone"> } => h.audience !== "everyone")
								.map((h) => (
									<li key={h.field} className="flex items-center gap-2.5 text-body text-fg-muted">
										<Lock aria-hidden strokeWidth={1.75} className="size-4 shrink-0" />
										{AUDIENCE_TEXT[h.audience](FIELD_NAME[h.field](), first)}
									</li>
								))}
						</ul>
					) : null}
				</>
			)}
		</article>
	);
}

function FriendAction({ profile }: { profile: PublicProfile }) {
	const [removing, setRemoving] = useState(false);
	const add = useAction((h: string, api) => api.friends.sendRequest(h), {
		success: m.toast_request_sent({ handle: profile.handle }),
		invalidate: [qk.profile(profile.handle), qk.friends],
	});
	const remove = useAction(
		async (_: void, api) => {
			const overview = await api.friends.overview();
			const f = overview.friends.find((x) => x.handle === profile.handle);
			if (f) await api.friends.remove(f.id);
		},
		{
			success: m.toast_friend_removed(),
			invalidate: [qk.profile(profile.handle), qk.friends],
			onSuccess: () => setRemoving(false),
		},
	);

	switch (profile.relation) {
		case "self":
			return (
				<Link to="/account/profile" className={buttonClass({ variant: "secondary" })}>
					<PencilLine aria-hidden />
					{m.overview_edit_profile()}
				</Link>
			);
		case "signed_out":
			return (
				<Link
					to="/sign-in"
					search={{ next: `/u/${profile.handle}` }}
					className={buttonClass({ variant: "primary" })}
				>
					<UserPlus aria-hidden />
					{m.public_signin_to_add()}
				</Link>
			);
		case "friend":
			return (
				<>
					<Menu>
						<MenuTrigger
							className={buttonClass({ variant: "secondary" })}
							aria-label={m.public_friends_menu({ name: profile.displayName })}
						>
							<Check aria-hidden />
							{m.public_friends()}
						</MenuTrigger>
						<MenuContent>
							<MenuItem onClick={() => setRemoving(true)}>
								<UserMinus aria-hidden strokeWidth={1.75} />
								{m.friends_remove()}
							</MenuItem>
						</MenuContent>
					</Menu>
					<ConfirmDialog
						open={removing}
						onOpenChange={setRemoving}
						title={m.friends_remove_title()}
						description={m.friends_remove_text({ name: profile.displayName })}
						confirmLabel={m.friends_remove_confirm()}
						pending={remove.isPending}
						onConfirm={() => remove.mutate()}
					/>
				</>
			);
		case "stranger":
			if (profile.request === "outgoing") {
				return (
					<Button variant="secondary" disabled>
						<Check aria-hidden />
						{m.public_request_sent()}
					</Button>
				);
			}
			if (profile.request === "incoming") {
				return (
					<Link to="/account/friends" className={buttonClass({ variant: "primary" })}>
						{m.public_answer_request()}
					</Link>
				);
			}
			return (
				<Button
					variant="primary"
					onClick={() => add.mutate(profile.handle)}
					loading={add.isPending}
				>
					<UserPlus aria-hidden />
					{m.public_add_friend()}
				</Button>
			);
	}
}
