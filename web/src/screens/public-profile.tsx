import { Link } from "@tanstack/react-router";
import { Ban, Check, Lock, PencilLine, UserMinus, UserPlus, UserRoundX } from "lucide-react";
import { useState } from "react";
import { describeError, isApiError } from "@/api/errors";
import { qk, useAction, usePublicUser, useSession } from "@/api/queries";
import type { PublicUser } from "@/api/types";
import { AppearanceMenu } from "@/components/app-shell";
import { Wordmark } from "@/components/logo";
import { ProfileBanner } from "@/components/profile-banner";
import { GradeTiles } from "@/components/trophy-parts";
import { m } from "@/i18n";
import { formatPlaytime } from "@/lib/format";
import { accentFor, avatarUrl, themeFor } from "@/lib/identity";
import { total } from "@/lib/trophies";
import { usePageTitle } from "@/lib/use-page-title";
import { Avatar } from "@/ui/avatar";
import { Button, buttonClass } from "@/ui/button";
import { ConfirmDialog } from "@/ui/dialog";
import { ErrorState, LoadingRegion, Skeleton } from "@/ui/feedback";
import { Menu, MenuContent, MenuItem, MenuTrigger } from "@/ui/menu";

export function PublicProfileScreen({ name }: { name: string }) {
	const user = usePublicUser(name);
	const session = useSession();
	const signedIn = session.data?.signedIn ?? false;
	usePageTitle(`@${user.data?.account.handle ?? name}`);

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
				{user.isPending ? (
					<ProfileSkeleton />
				) : user.isError ? (
					isApiError(user.error) && user.error.code === "not_found" ? (
						<NotFound name={name} />
					) : (
						<div className="rounded-xl bg-surface shadow-card">
							<ErrorState
								title={m.public_error_title()}
								description={describeError(user.error)}
								onRetry={() => void user.refetch()}
								retrying={user.isFetching}
							/>
						</div>
					)
				) : (
					<ProfileView user={user.data} />
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

function ProfileView({ user }: { user: PublicUser }) {
	const handle = user.account.handle;
	const relation = user.viewer?.relation ?? "none";
	const hasAnything = Boolean(
		user.about || user.favourites || user.playtime || user.trophies || user.avatar,
	);
	return (
		<article className="flex flex-col gap-6" aria-label={`@${handle}`}>
			<div className="overflow-hidden rounded-2xl bg-surface shadow-card">
				<ProfileBanner theme={themeFor(handle)} className="h-40 sm:h-48" />
				<div className="px-5 pb-6 sm:px-6">
					<div className="flex items-start justify-between gap-4">
						<Avatar
							name={handle}
							accent={accentFor(handle)}
							src={avatarUrl(user.avatar)}
							size="xl"
							className="-mt-12 rounded-full ring-4 ring-surface"
						/>
						<div className="pt-3">
							<FriendAction user={user} />
						</div>
					</div>
					<h1 className="mt-4 font-mono text-display font-semibold text-fg">@{handle}</h1>
					{user.about ? <p className="mt-3 max-w-[56ch] text-prose text-fg">{user.about}</p> : null}
				</div>
			</div>

			{user.favourites && user.favourites.length > 0 ? (
				<section aria-labelledby="pp-fav" className="flex flex-col gap-3">
					<h2 id="pp-fav" className="text-prose font-semibold">
						{m.public_favourites()}
					</h2>
					<ul className="flex flex-wrap gap-2">
						{user.favourites.map((t) => (
							<li
								key={t}
								className="rounded-full bg-raised px-3 py-1.5 font-mono text-small text-fg shadow-[inset_0_0_0_1px_var(--line-strong)]"
							>
								{t}
							</li>
						))}
					</ul>
				</section>
			) : null}

			{user.trophies ? (
				<section aria-labelledby="pp-trophies" className="flex flex-col gap-3">
					<h2 id="pp-trophies" className="text-prose font-semibold">
						{m.public_trophies_title()}
						<span className="ms-2 text-body font-normal text-fg-muted">
							{m.public_trophies_total({ count: total(user.trophies) })}
						</span>
					</h2>
					<GradeTiles counts={user.trophies} />
				</section>
			) : null}

			{user.playtime && user.playtime.length > 0 ? (
				<section aria-labelledby="pp-play" className="flex flex-col gap-3">
					<h2 id="pp-play" className="text-prose font-semibold">
						{m.profile_playtime()}
					</h2>
					<ul className="divide-y divide-line overflow-hidden rounded-xl bg-surface shadow-card">
						{user.playtime.map((p) => (
							<li key={p.titleId} className="flex items-center justify-between gap-4 px-5 py-3.5">
								<span className="font-mono text-small text-fg">{p.titleId}</span>
								<span className="nums text-body text-fg-muted">{formatPlaytime(p.minutes)}</span>
							</li>
						))}
					</ul>
				</section>
			) : null}

			{relation !== "self" ? (
				<p className="flex items-start gap-2.5 text-body text-fg-muted">
					<Lock aria-hidden strokeWidth={1.75} className="mt-0.5 size-4 shrink-0" />
					{hasAnything
						? user.viewer
							? m.public_more_friends({ name: `@${handle}` })
							: m.public_more_signed_out({ name: `@${handle}` })
						: user.viewer
							? m.public_nothing_shared({ name: `@${handle}` })
							: m.public_nothing_shared_signed_out({ name: `@${handle}` })}
				</p>
			) : null}
		</article>
	);
}

function FriendAction({ user }: { user: PublicUser }) {
	const handle = user.account.handle;
	const [removing, setRemoving] = useState(false);
	const [blocking, setBlocking] = useState(false);
	const refresh = [qk.user(handle), qk.friends];
	const add = useAction((h: string, api) => api.friends.sendRequest(h), {
		success: (state) =>
			state === "friends" ? m.toast_now_friends({ handle }) : m.toast_request_sent({ handle }),
		invalidate: refresh,
	});
	const remove = useAction((_: undefined, api) => api.friends.remove(user.account.id), {
		success: m.toast_friend_removed(),
		invalidate: refresh,
		onSuccess: () => setRemoving(false),
	});
	const block = useAction((_: undefined, api) => api.friends.block(user.account.id), {
		success: m.toast_blocked(),
		invalidate: refresh,
		onSuccess: () => setBlocking(false),
	});
	const unblock = useAction((_: undefined, api) => api.friends.unblock(user.account.id), {
		success: m.toast_unblocked(),
		invalidate: refresh,
	});

	const viewer = user.viewer;
	if (!viewer) {
		return (
			<Link
				to="/sign-in"
				search={{ next: `/u/${handle}` }}
				className={buttonClass({ variant: "primary" })}
			>
				<UserPlus aria-hidden />
				{m.public_signin_to_add()}
			</Link>
		);
	}
	if (viewer.relation === "self") {
		return (
			<Link to="/account/profile" className={buttonClass({ variant: "secondary" })}>
				<PencilLine aria-hidden />
				{m.overview_edit_profile()}
			</Link>
		);
	}
	if (viewer.blocked) {
		return (
			<Button
				variant="secondary"
				onClick={() => unblock.mutate(undefined)}
				loading={unblock.isPending}
			>
				<Ban aria-hidden />
				{m.blocked_unblock()}
			</Button>
		);
	}
	if (viewer.relation === "friend") {
		return (
			<>
				<Menu>
					<MenuTrigger className={buttonClass({ variant: "secondary" })}>
						<Check aria-hidden />
						{m.public_friends()}
						<span className="sr-only"> {m.public_friends_with({ name: `@${handle}` })}</span>
					</MenuTrigger>
					<MenuContent>
						<MenuItem onClick={() => setRemoving(true)}>
							<UserMinus aria-hidden strokeWidth={1.75} />
							{m.friends_remove()}
						</MenuItem>
						<MenuItem onClick={() => setBlocking(true)}>
							<Ban aria-hidden strokeWidth={1.75} />
							{m.friends_block()}
						</MenuItem>
					</MenuContent>
				</Menu>
				<ConfirmDialog
					open={removing}
					onOpenChange={setRemoving}
					title={m.friends_remove_title()}
					description={m.friends_remove_text({ name: `@${handle}` })}
					confirmLabel={m.friends_remove_confirm()}
					pending={remove.isPending}
					onConfirm={() => remove.mutate(undefined)}
				/>
				<ConfirmDialog
					open={blocking}
					onOpenChange={setBlocking}
					title={m.friends_block_title()}
					description={m.friends_block_text({ name: `@${handle}` })}
					confirmLabel={m.friends_block_confirm()}
					pending={block.isPending}
					onConfirm={() => block.mutate(undefined)}
				/>
			</>
		);
	}
	if (viewer.requested === "outgoing") {
		return (
			<Button variant="secondary" disabled>
				<Check aria-hidden />
				{m.public_request_sent()}
			</Button>
		);
	}
	if (viewer.requested === "incoming") {
		return (
			<Link to="/account/friends" className={buttonClass({ variant: "primary" })}>
				{m.public_answer_request()}
			</Link>
		);
	}
	return (
		<Button variant="primary" onClick={() => add.mutate(handle)} loading={add.isPending}>
			<UserPlus aria-hidden />
			{m.public_add_friend()}
		</Button>
	);
}
