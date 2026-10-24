import { Link } from "@tanstack/react-router";
import {
	Ban,
	Check,
	Ellipsis,
	ExternalLink,
	UserMinus,
	UserPlus,
	UsersRound,
	X,
} from "lucide-react";
import { useState } from "react";
import { describeError, isApiError } from "@/api/errors";
import { qk, useAction, useFriends } from "@/api/queries";
import type { Account, Friend, FriendRequest } from "@/api/types";
import { AnimatedRow, AnimatedRows } from "@/components/animated-rows";
import { Page, PageHeader, Section } from "@/components/layout";
import { PersonHoverCard } from "@/components/person-card";
import { QueryView } from "@/components/query-state";
import { m } from "@/i18n";
import { formatMonthYear, relativeTime } from "@/lib/format";
import { accentFor } from "@/lib/identity";
import { usePageTitle } from "@/lib/use-page-title";
import { Avatar, AvatarGroup } from "@/ui/avatar";
import { Badge } from "@/ui/badge";
import { Button } from "@/ui/button";
import { ConfirmDialog } from "@/ui/dialog";
import { EmptyState, Skeleton } from "@/ui/feedback";
import { Field } from "@/ui/field";
import { Input } from "@/ui/input";
import { Menu, MenuContent, MenuItem, MenuTrigger } from "@/ui/menu";
import { Tab, Tabs, TabsList, TabsPanel } from "@/ui/tabs";

type TabId = "friends" | "requests" | "blocked";

export function Friends() {
	usePageTitle(m.nav_friends());
	const friends = useFriends();
	const incoming = friends.data?.incoming.length ?? 0;
	const [tab, setTab] = useState<TabId>("friends");
	return (
		<Page width="wide">
			<PageHeader title={m.friends_title()} description={m.friends_lead()} />
			<AddFriend />
			<Tabs value={tab} onValueChange={(v) => setTab(v as TabId)}>
				<TabsList aria-label={m.friends_title()}>
					<Tab value="friends">
						{m.friends_tab_friends()}
						{friends.data ? <Count n={friends.data.friends.length} /> : null}
					</Tab>
					<Tab value="requests">
						{m.friends_tab_requests()}
						{friends.data ? (
							<Count n={incoming + friends.data.outgoing.length} highlight={incoming > 0} />
						) : null}
					</Tab>
					<Tab value="blocked">
						{m.friends_tab_blocked()}
						{friends.data && friends.data.blocked.length > 0 ? (
							<Count n={friends.data.blocked.length} />
						) : null}
					</Tab>
				</TabsList>
				<TabsPanel value="friends" className="pt-6 outline-none">
					<FriendsList onAdd={() => document.getElementById("add-friend-handle")?.focus()} />
				</TabsPanel>
				<TabsPanel value="requests" className="pt-6 outline-none">
					<Requests />
				</TabsPanel>
				<TabsPanel value="blocked" className="pt-6 outline-none">
					<Blocked />
				</TabsPanel>
			</Tabs>
		</Page>
	);
}

function Count({ n, highlight }: { n: number; highlight?: boolean }) {
	return (
		<span
			className={`nums rounded-full px-1.5 text-caption font-semibold ${highlight ? "bg-accent-soft text-accent-text" : "bg-raised text-fg-muted"}`}
		>
			{n}
		</span>
	);
}

function AddFriend() {
	const [handle, setHandle] = useState("");
	const [error, setError] = useState<string | null>(null);
	const send = useAction((h: string, api) => api.friends.sendRequest(h), {
		success: (state, h) =>
			state === "friends"
				? m.toast_now_friends({ handle: h.replace(/^@/, "") })
				: m.toast_request_sent({ handle: h.replace(/^@/, "") }),
		invalidate: [qk.friends],
		toastErrors: false,
		onSuccess: () => setHandle(""),
	});
	return (
		<Section title={m.friends_add_title()} description={m.friends_add_lead()}>
			<form
				noValidate
				className="flex flex-col gap-3 sm:flex-row sm:items-start"
				onSubmit={(e) => {
					e.preventDefault();
					const h = handle.trim().replace(/^@/, "");
					if (!h) {
						setError(m.friends_add_empty());
						return;
					}
					setError(null);
					send.mutate(h, {
						onError: (err) =>
							setError(
								isApiError(err) && err.fields.handle ? err.fields.handle : describeError(err),
							),
					});
				}}
			>
				<Field label={m.friends_add_label()} error={error} className="flex-1" name="handle">
					<div className="relative">
						<span
							aria-hidden
							className="pointer-events-none absolute inset-y-0 start-3 flex items-center font-mono text-body text-fg-muted"
						>
							@
						</span>
						<Input
							id="add-friend-handle"
							value={handle}
							onChange={(e) => {
								setHandle(e.target.value);
								setError(null);
							}}
							autoCapitalize="none"
							autoComplete="off"
							spellCheck={false}
							className="ps-7 font-mono"
							placeholder={m.friends_add_placeholder()}
						/>
					</div>
				</Field>
				<div className="sm:pt-[1.625rem]">
					<Button
						type="submit"
						variant="primary"
						loading={send.isPending}
						className="w-full sm:w-auto"
					>
						<UserPlus aria-hidden />
						{m.friends_add_button()}
					</Button>
				</div>
			</form>
		</Section>
	);
}

function PersonCell({
	account,
	online,
	friend,
	children,
}: {
	account: Account;
	online?: boolean;
	friend?: Friend;
	children?: React.ReactNode;
}) {
	return (
		<div className="flex min-w-0 flex-1 items-center gap-3.5">
			<Avatar
				name={account.handle}
				accent={accentFor(account.handle)}
				size="md"
				status={online === undefined ? null : online ? "online" : "offline"}
			/>
			<div className="min-w-0">
				<p className="truncate font-mono text-body font-medium text-fg">
					{friend ? (
						<PersonHoverCard friend={friend}>@{account.handle}</PersonHoverCard>
					) : (
						<Link
							to="/u/$name"
							params={{ name: account.handle }}
							className="rounded-sm outline-offset-2 hover:underline hover:decoration-line-strong hover:underline-offset-4"
						>
							@{account.handle}
						</Link>
					)}
				</p>
				<p className="truncate text-small text-fg-muted">{children}</p>
			</div>
		</div>
	);
}

function ListSkeleton() {
	return (
		<ul>
			{["a", "b", "c"].map((i) => (
				<li
					key={i}
					className="flex items-center gap-3.5 border-t border-line px-5 py-4 first:border-t-0"
				>
					<Skeleton className="size-10 rounded-full" />
					<div className="space-y-2">
						<Skeleton className="h-4 w-36" />
						<Skeleton className="h-3 w-24" />
					</div>
				</li>
			))}
		</ul>
	);
}

const statusText = (f: Friend) =>
	f.presence.activity
		? m.friends_playing({ title: f.presence.activity.title })
		: f.presence.online
			? m.status_online()
			: m.status_offline();

function FriendsList({ onAdd }: { onAdd: () => void }) {
	const friends = useFriends();
	const [removing, setRemoving] = useState<Friend | null>(null);
	const [blocking, setBlocking] = useState<Friend | null>(null);
	const remove = useAction((id: number, api) => api.friends.remove(id), {
		success: m.toast_friend_removed(),
		invalidate: [qk.friends],
		onSuccess: () => setRemoving(null),
	});
	const block = useAction((id: number, api) => api.friends.block(id), {
		success: m.toast_blocked(),
		invalidate: [qk.friends],
		onSuccess: () => setBlocking(null),
	});
	return (
		<div className="overflow-hidden rounded-xl bg-surface shadow-card">
			<QueryView
				query={friends}
				loadingLabel={m.friends_loading()}
				loading={<ListSkeleton />}
				errorTitle={m.friends_error()}
				isEmpty={(d) => d.friends.length === 0}
				empty={
					<EmptyState
						icon={<UsersRound aria-hidden strokeWidth={1.75} />}
						title={m.friends_empty_title()}
						description={m.friends_empty_text()}
						action={
							<Button variant="secondary" onClick={onAdd}>
								<UserPlus aria-hidden />
								{m.friends_empty_action()}
							</Button>
						}
					/>
				}
			>
				{(d) => (
					<>
						<OnlineStrip friends={d.friends} />
						<AnimatedRows label={m.friends_tab_friends()}>
							{[...d.friends]
								.sort(
									(a, b) =>
										Number(b.presence.online) - Number(a.presence.online) ||
										a.account.handle.localeCompare(b.account.handle),
								)
								.map((f) => (
									<AnimatedRow key={f.account.id}>
										<div className="flex items-center gap-3 px-5 py-3.5">
											<PersonCell account={f.account} online={f.presence.online} friend={f}>
												{statusText(f)}
											</PersonCell>
											<span className="hidden shrink-0 text-small text-fg-muted lg:block">
												{m.friends_since({ date: formatMonthYear(f.since) })}
											</span>
											<Menu>
												<MenuTrigger
													aria-label={m.friends_actions({ name: f.account.handle })}
													className="press flex size-9 shrink-0 items-center justify-center rounded-md text-fg-muted outline-offset-0 pointer-hover:hover:bg-hover pointer-hover:hover:text-fg data-[popup-open]:bg-hover coarse:size-11"
												>
													<Ellipsis aria-hidden className="size-4" />
												</MenuTrigger>
												<MenuContent>
													<MenuItem
														render={<Link to="/u/$name" params={{ name: f.account.handle }} />}
													>
														<ExternalLink aria-hidden strokeWidth={1.75} />
														{m.menu_view_profile()}
													</MenuItem>
													<MenuItem onClick={() => setRemoving(f)}>
														<UserMinus aria-hidden strokeWidth={1.75} />
														{m.friends_remove()}
													</MenuItem>
													<MenuItem onClick={() => setBlocking(f)}>
														<Ban aria-hidden strokeWidth={1.75} />
														{m.friends_block()}
													</MenuItem>
												</MenuContent>
											</Menu>
										</div>
									</AnimatedRow>
								))}
						</AnimatedRows>
					</>
				)}
			</QueryView>
			<ConfirmDialog
				open={removing !== null}
				onOpenChange={(o) => !o && setRemoving(null)}
				title={m.friends_remove_title()}
				description={
					removing ? m.friends_remove_text({ name: `@${removing.account.handle}` }) : undefined
				}
				confirmLabel={m.friends_remove_confirm()}
				pending={remove.isPending}
				onConfirm={() => removing && remove.mutate(removing.account.id)}
			/>
			<ConfirmDialog
				open={blocking !== null}
				onOpenChange={(o) => !o && setBlocking(null)}
				title={m.friends_block_title()}
				description={
					blocking ? m.friends_block_text({ name: `@${blocking.account.handle}` }) : undefined
				}
				confirmLabel={m.friends_block_confirm()}
				pending={block.isPending}
				onConfirm={() => blocking && block.mutate(blocking.account.id)}
			/>
		</div>
	);
}

function OnlineStrip({ friends }: { friends: Friend[] }) {
	const online = friends.filter((f) => f.presence.online);
	return (
		<div className="flex items-center gap-3 border-b border-line px-5 py-3">
			{online.length > 0 ? (
				<AvatarGroup
					people={online.map((f) => ({
						name: f.account.handle,
						accent: accentFor(f.account.handle),
					}))}
					label={m.friends_online_group({ count: online.length })}
					size="sm"
				/>
			) : null}
			<p className="text-small text-fg-muted">
				{online.length > 0
					? m.friends_online_group({ count: online.length })
					: m.friends_online_none()}
			</p>
		</div>
	);
}

function Requests() {
	const friends = useFriends();
	return (
		<QueryView
			query={friends}
			loadingLabel={m.friends_loading()}
			loading={
				<div className="overflow-hidden rounded-xl bg-surface shadow-card">
					<ListSkeleton />
				</div>
			}
			errorTitle={m.friends_error()}
			isEmpty={(d) => d.incoming.length + d.outgoing.length === 0}
			empty={
				<div className="overflow-hidden rounded-xl bg-surface shadow-card">
					<EmptyState title={m.requests_empty_title()} description={m.requests_empty_text()} />
				</div>
			}
		>
			{(d) => (
				<div className="flex flex-col gap-10">
					{d.incoming.length > 0 ? (
						<Section title={m.requests_incoming()} flush>
							<AnimatedRows label={m.requests_incoming()}>
								{d.incoming.map((r) => (
									<AnimatedRow key={r.account.id}>
										<IncomingRow request={r} />
									</AnimatedRow>
								))}
							</AnimatedRows>
						</Section>
					) : null}
					{d.outgoing.length > 0 ? (
						<Section title={m.requests_outgoing()} description={m.requests_outgoing_lead()} flush>
							<AnimatedRows label={m.requests_outgoing()}>
								{d.outgoing.map((r) => (
									<AnimatedRow key={r.account.id}>
										<OutgoingRow request={r} />
									</AnimatedRow>
								))}
							</AnimatedRows>
						</Section>
					) : null}
				</div>
			)}
		</QueryView>
	);
}

function IncomingRow({ request }: { request: FriendRequest }) {
	const handle = request.account.handle;
	const accept = useAction((id: number, api) => api.friends.accept(id), {
		success: m.toast_request_accepted({ name: `@${handle}` }),
		invalidate: [qk.friends],
	});
	const decline = useAction((id: number, api) => api.friends.dropRequest(id), {
		invalidate: [qk.friends],
	});
	const busy = accept.isPending || decline.isPending;
	return (
		<div className="flex flex-col gap-3 px-5 py-4 sm:flex-row sm:items-center sm:gap-4">
			<PersonCell account={request.account}>{relativeTime(request.since)}</PersonCell>
			<div className="flex shrink-0 gap-3">
				<Button
					variant="primary"
					size="sm"
					onClick={() => accept.mutate(request.account.id)}
					loading={accept.isPending}
					disabled={busy}
				>
					<Check aria-hidden />
					{m.requests_accept()}
					<span className="sr-only"> @{handle}</span>
				</Button>
				<Button
					variant="secondary"
					size="sm"
					onClick={() => decline.mutate(request.account.id)}
					loading={decline.isPending}
					disabled={busy}
				>
					{m.requests_decline()}
					<span className="sr-only"> @{handle}</span>
				</Button>
			</div>
		</div>
	);
}

function OutgoingRow({ request }: { request: FriendRequest }) {
	const cancel = useAction((id: number, api) => api.friends.dropRequest(id), {
		success: m.toast_request_cancelled(),
		invalidate: [qk.friends],
	});
	return (
		<div className="flex items-center gap-3 px-5 py-4">
			<PersonCell account={request.account}>
				{m.requests_sent({ when: relativeTime(request.since) })}
			</PersonCell>
			<Badge>{m.requests_pending()}</Badge>
			<Button
				variant="ghost"
				size="sm"
				onClick={() => cancel.mutate(request.account.id)}
				loading={cancel.isPending}
				aria-label={m.requests_cancel_named({ name: `@${request.account.handle}` })}
			>
				<X aria-hidden />
				{m.common_cancel()}
			</Button>
		</div>
	);
}

function Blocked() {
	const friends = useFriends();
	const unblock = useAction((id: number, api) => api.friends.unblock(id), {
		success: m.toast_unblocked(),
		invalidate: [qk.friends],
	});
	return (
		<div className="overflow-hidden rounded-xl bg-surface shadow-card">
			<QueryView
				query={friends}
				loadingLabel={m.friends_loading()}
				loading={<ListSkeleton />}
				errorTitle={m.friends_error()}
				isEmpty={(d) => d.blocked.length === 0}
				empty={<EmptyState title={m.blocked_empty_title()} description={m.blocked_empty_text()} />}
			>
				{(d) => (
					<AnimatedRows label={m.friends_tab_blocked()}>
						{d.blocked.map((a) => (
							<AnimatedRow key={a.id}>
								<div className="flex items-center gap-3 px-5 py-3.5">
									<PersonCell account={a}>{m.blocked_note()}</PersonCell>
									<Button
										variant="secondary"
										size="sm"
										onClick={() => unblock.mutate(a.id)}
										aria-label={m.blocked_unblock_named({ name: `@${a.handle}` })}
									>
										{m.blocked_unblock()}
									</Button>
								</div>
							</AnimatedRow>
						))}
					</AnimatedRows>
				)}
			</QueryView>
		</div>
	);
}
