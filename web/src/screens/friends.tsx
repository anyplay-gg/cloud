import { Link } from "@tanstack/react-router";
import { usePageTitle } from "@/lib/use-page-title";
import { Check, Ellipsis, ExternalLink, UserMinus, UserPlus, UsersRound, X } from "lucide-react";
import { useState } from "react";
import { describeError, isApiError } from "@/api/errors";
import { qk, useAction, useFriends } from "@/api/queries";
import type { Friend, FriendRequest, Person } from "@/api/types";
import { AnimatedRow, AnimatedRows } from "@/components/animated-rows";
import { Page, PageHeader, Section } from "@/components/layout";
import { QueryView } from "@/components/query-state";
import { m } from "@/i18n";
import { formatMonthYear, relativeTime } from "@/lib/format";
import { Avatar } from "@/ui/avatar";
import { Badge } from "@/ui/badge";
import { Button } from "@/ui/button";
import { ConfirmDialog } from "@/ui/dialog";
import { EmptyState, Skeleton } from "@/ui/feedback";
import { Field } from "@/ui/field";
import { Input } from "@/ui/input";
import { Menu, MenuContent, MenuItem, MenuTrigger } from "@/ui/menu";
import { Tab, Tabs, TabsList, TabsPanel } from "@/ui/tabs";

export function Friends() {
	usePageTitle(m.nav_friends());
	const friends = useFriends();
	const incoming = friends.data?.incoming.length ?? 0;
	const [tab, setTab] = useState<"friends" | "requests">("friends");
	return (
		<Page width="wide">
			<PageHeader title={m.friends_title()} description={m.friends_lead()} />
			<AddFriend />
			<Tabs value={tab} onValueChange={(v) => setTab(v as typeof tab)}>
				<TabsList aria-label={m.friends_title()}>
					<Tab value="friends">
						{m.friends_tab_friends()}
						{friends.data ? <Count n={friends.data.friends.length} /> : null}
					</Tab>
					<Tab value="requests">
						{m.friends_tab_requests()}
						{friends.data ? <Count n={incoming + friends.data.outgoing.length} highlight={incoming > 0} /> : null}
					</Tab>
				</TabsList>
				<TabsPanel value="friends" className="pt-6 outline-none">
					<FriendsList onAdd={() => document.getElementById("add-friend-handle")?.focus()} />
				</TabsPanel>
				<TabsPanel value="requests" className="pt-6 outline-none">
					<Requests />
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
		success: (_, h) => m.toast_request_sent({ handle: h.replace(/^@/, "") }),
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
						onError: (err) => {
							if (isApiError(err) && err.fields.handle) setError(err.fields.handle);
							else if (isApiError(err) && err.code === "rate_limited") setError(describeError(err));
							else setError(describeError(err));
						},
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
					<Button type="submit" variant="primary" loading={send.isPending} className="w-full sm:w-auto">
						<UserPlus aria-hidden />
						{m.friends_add_button()}
					</Button>
				</div>
			</form>
		</Section>
	);
}

function PersonCell({
	person,
	status,
	children,
}: {
	person: Person;
	status?: Friend["status"];
	children?: React.ReactNode;
}) {
	return (
		<div className="flex min-w-0 flex-1 items-center gap-3.5">
			<Avatar name={person.displayName} accent={person.accent} size="md" status={status ?? null} />
			<div className="min-w-0">
				<p className="truncate text-body font-medium text-fg">{person.displayName}</p>
				<p className="truncate text-small text-fg-muted">
					<span className="font-mono">@{person.handle}</span>
					{children}
				</p>
			</div>
		</div>
	);
}

function ListSkeleton() {
	return (
		<ul>
			{[0, 1, 2].map((i) => (
				<li key={i} className="flex items-center gap-3.5 border-t border-line px-5 py-4 first:border-t-0">
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

function FriendsList({ onAdd }: { onAdd: () => void }) {
	const friends = useFriends();
	const [removing, setRemoving] = useState<Friend | null>(null);
	const remove = useAction((id: string, api) => api.friends.remove(id), {
		success: m.toast_friend_removed(),
		invalidate: [qk.friends],
		onSuccess: () => setRemoving(null),
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
					<AnimatedRows label={m.friends_tab_friends()}>
						{[...d.friends]
							.sort((a, b) => rank(a.status) - rank(b.status) || a.displayName.localeCompare(b.displayName))
							.map((f) => (
								<AnimatedRow key={f.id}>
									<div className="flex items-center gap-3 px-5 py-3.5">
										<PersonCell person={f} status={f.status}>
											{" · "}
											{f.activity ?? statusText(f.status)}
										</PersonCell>
										<span className="hidden shrink-0 text-small text-fg-muted lg:block">
											{m.friends_since({ date: formatMonthYear(f.since) })}
										</span>
										<Menu>
											<MenuTrigger
												aria-label={m.friends_actions({ name: f.displayName })}
												className="press flex size-9 shrink-0 items-center justify-center rounded-md text-fg-muted outline-offset-0 pointer-hover:hover:bg-hover pointer-hover:hover:text-fg data-[popup-open]:bg-hover coarse:size-11"
											>
												<Ellipsis aria-hidden className="size-4" />
											</MenuTrigger>
											<MenuContent>
												<MenuItem render={<Link to="/u/$name" params={{ name: f.handle }} />}>
													<ExternalLink aria-hidden strokeWidth={1.75} />
													{m.menu_view_profile()}
												</MenuItem>
												<MenuItem onClick={() => setRemoving(f)}>
													<UserMinus aria-hidden strokeWidth={1.75} />
													{m.friends_remove()}
												</MenuItem>
											</MenuContent>
										</Menu>
									</div>
								</AnimatedRow>
							))}
					</AnimatedRows>
				)}
			</QueryView>
			<ConfirmDialog
				open={removing !== null}
				onOpenChange={(o) => !o && setRemoving(null)}
				title={m.friends_remove_title()}
				description={removing ? m.friends_remove_text({ name: removing.displayName }) : undefined}
				confirmLabel={m.friends_remove_confirm()}
				pending={remove.isPending}
				onConfirm={() => removing && remove.mutate(removing.id)}
			/>
		</div>
	);
}

const rank = (s: Friend["status"]) => (s === "online" ? 0 : s === "away" ? 1 : 2);
const statusText = (s: Friend["status"]) =>
	s === "online" ? m.status_online() : s === "away" ? m.status_away() : m.status_offline();

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
				<div className="flex flex-col gap-8">
					{d.incoming.length > 0 ? (
						<Section title={m.requests_incoming()} flush>
							<AnimatedRows label={m.requests_incoming()}>
								{d.incoming.map((r) => (
									<AnimatedRow key={r.id}>
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
									<AnimatedRow key={r.id}>
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
	const accept = useAction((id: string, api) => api.friends.accept(id), {
		success: m.toast_request_accepted({ name: request.person.displayName }),
		invalidate: [qk.friends],
	});
	const decline = useAction((id: string, api) => api.friends.decline(id), {
		invalidate: [qk.friends],
	});
	const busy = accept.isPending || decline.isPending;
	return (
		<div className="flex flex-col gap-3 px-5 py-4 sm:flex-row sm:items-center sm:gap-4">
			<PersonCell person={request.person}>
				{" · "}
				{relativeTime(request.createdAt)}
			</PersonCell>
			<div className="flex shrink-0 gap-3">
				<Button
					variant="primary"
					size="sm"
					onClick={() => accept.mutate(request.id)}
					loading={accept.isPending}
					disabled={busy}
				>
					<Check aria-hidden />
					{m.requests_accept()}
					<span className="sr-only"> {request.person.displayName}</span>
				</Button>
				<Button
					variant="secondary"
					size="sm"
					onClick={() => decline.mutate(request.id)}
					loading={decline.isPending}
					disabled={busy}
				>
					{m.requests_decline()}
					<span className="sr-only"> {request.person.displayName}</span>
				</Button>
			</div>
		</div>
	);
}

function OutgoingRow({ request }: { request: FriendRequest }) {
	const cancel = useAction((id: string, api) => api.friends.cancel(id), {
		success: m.toast_request_cancelled(),
		invalidate: [qk.friends],
	});
	return (
		<div className="flex items-center gap-3 px-5 py-4">
			<PersonCell person={request.person}>
				{" · "}
				{m.requests_sent({ when: relativeTime(request.createdAt) })}
			</PersonCell>
			<Badge>{m.requests_pending()}</Badge>
			<Button
				variant="ghost"
				size="sm"
				onClick={() => cancel.mutate(request.id)}
				loading={cancel.isPending}
				aria-label={m.requests_cancel_named({ name: request.person.displayName })}
			>
				<X aria-hidden />
				{m.common_cancel()}
			</Button>
		</div>
	);
}
