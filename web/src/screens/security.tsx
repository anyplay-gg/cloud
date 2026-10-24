import { useQueryClient } from "@tanstack/react-query";
import { usePageTitle } from "@/lib/use-page-title";
import { Globe, KeyRound, Mail, Monitor, Plus } from "lucide-react";
import { useState } from "react";
import { describeError, isApiError } from "@/api/errors";
import { qk, useAction, useDevices, usePasskeys, useSession } from "@/api/queries";
import type { DeviceSession, Passkey } from "@/api/types";
import { AnimatedRow, AnimatedRows } from "@/components/animated-rows";
import { Page, PageHeader, Row, Section } from "@/components/layout";
import { QueryView } from "@/components/query-state";
import { isStepUpCancelled } from "@/features/step-up";
import { m } from "@/i18n";
import { formatDate, relativeTime } from "@/lib/format";
import { Badge } from "@/ui/badge";
import { Button } from "@/ui/button";
import { ConfirmDialog, Dialog, DialogContent } from "@/ui/dialog";
import { EmptyState, Skeleton } from "@/ui/feedback";
import { Field } from "@/ui/field";
import { Input } from "@/ui/input";

const MAX_PASSKEYS = 10;

export function Security() {
	usePageTitle(m.nav_security());
	return (
		<Page>
			<PageHeader title={m.security_title()} description={m.security_lead()} />
			<PasskeysSection />
			<EmailSection />
			<DevicesSection />
		</Page>
	);
}

function RowSkeleton() {
	return (
		<div className="flex items-center gap-4 px-5 py-4">
			<Skeleton className="size-10 rounded-lg" />
			<div className="flex-1 space-y-2">
				<Skeleton className="h-4 w-40" />
				<Skeleton className="h-3 w-64 max-w-full" />
			</div>
		</div>
	);
}

function IconTile({ children }: { children: React.ReactNode }) {
	return (
		<span className="flex size-10 shrink-0 items-center justify-center rounded-lg bg-raised text-fg-muted shadow-[inset_0_0_0_1px_var(--line-strong)] [&_svg]:size-[1.125rem]">
			{children}
		</span>
	);
}

// ---------------------------------------------------------------------------------------------
// Passkeys

function PasskeysSection() {
	const passkeys = usePasskeys();
	const [adding, setAdding] = useState(false);
	const [removing, setRemoving] = useState<Passkey | null>(null);
	const count = passkeys.data?.length ?? 0;
	const canAdd = !passkeys.isError && count < MAX_PASSKEYS;

	return (
		<Section
			id="passkeys"
			title={m.passkeys_title()}
			description={m.passkeys_lead({ max: MAX_PASSKEYS })}
			flush
			actions={
				canAdd && count > 0 ? (
					<Button variant="primary" size="sm" onClick={() => setAdding(true)}>
						<Plus aria-hidden />
						{m.passkeys_add()}
					</Button>
				) : null
			}
		>
			<QueryView
				query={passkeys}
				loadingLabel={m.passkeys_loading()}
				loading={<RowSkeleton />}
				errorTitle={m.passkeys_error()}
				isEmpty={(d) => d.length === 0}
				empty={
					<EmptyState
						icon={<KeyRound aria-hidden strokeWidth={1.75} />}
						title={m.passkeys_empty_title()}
						description={m.passkeys_empty_text()}
						action={
							<Button variant="primary" onClick={() => setAdding(true)}>
								<Plus aria-hidden />
								{m.passkeys_add_first()}
							</Button>
						}
					/>
				}
			>
				{(list) => (
					<AnimatedRows label={m.passkeys_title()}>
						{list.map((k) => (
							<AnimatedRow key={k.id}>
								<div className="flex items-center gap-4 px-5 py-4">
									<IconTile>
										<KeyRound aria-hidden strokeWidth={1.75} />
									</IconTile>
									<div className="min-w-0 flex-1">
										<p className="flex flex-wrap items-center gap-x-2 gap-y-1 text-body font-medium text-fg">
											<span className="truncate">{k.name}</span>
											{k.synced ? <Badge>{m.passkeys_synced()}</Badge> : null}
										</p>
										<p className="mt-0.5 text-small text-fg-muted">
											{m.passkeys_added({ date: formatDate(k.createdAt, "medium") })}
											{" · "}
											{k.lastUsedAt
												? m.passkeys_last_used({ when: relativeTime(k.lastUsedAt) })
												: m.passkeys_never_used()}
										</p>
									</div>
									<Button
										variant="ghost"
										size="sm"
										onClick={() => setRemoving(k)}
										aria-label={m.passkeys_remove_named({ name: k.name })}
									>
										{m.common_remove()}
									</Button>
								</div>
							</AnimatedRow>
						))}
					</AnimatedRows>
				)}
			</QueryView>
			<AddPasskeyDialog open={adding} onOpenChange={setAdding} />
			<RemovePasskeyDialog passkey={removing} onClose={() => setRemoving(null)} isLast={count === 1} />
		</Section>
	);
}

function guessDeviceName(): string {
	const ua = navigator.userAgent;
	const os = /Windows/.test(ua)
		? "Windows"
		: /Android/.test(ua)
			? "Android"
			: /iPhone|iPad/.test(ua)
				? "iOS"
				: /Mac OS/.test(ua)
					? "Mac"
					: /Linux/.test(ua)
						? "Linux"
						: "";
	return os ? m.passkeys_default_name({ device: os }) : m.passkeys_default_name_plain();
}

function AddPasskeyDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (o: boolean) => void }) {
	return (
		<Dialog open={open} onOpenChange={onOpenChange}>
			{open ? <AddPasskeyBody onDone={() => onOpenChange(false)} /> : null}
		</Dialog>
	);
}

function AddPasskeyBody({ onDone }: { onDone: () => void }) {
	const [name, setName] = useState(guessDeviceName);
	const [error, setError] = useState<string | null>(null);
	const add = useAction((n: string, api) => api.security.addPasskey(n), {
		success: m.toast_passkey_added(),
		invalidate: [qk.passkeys, qk.activity],
		toastErrors: false,
		onSuccess: onDone,
	});
	const submit = () => {
		setError(null);
		add.mutate(name, {
			onError: (e) => {
				if (isStepUpCancelled(e)) return;
				if (isApiError(e) && e.code === "passkey_cancelled") setError(m.passkeys_prompt_closed());
				else setError(describeError(e));
			},
		});
	};
	return (
		<DialogContent
			title={m.passkeys_add_title()}
			description={m.passkeys_add_text()}
			footer={
				<>
					<Button variant="secondary" onClick={onDone} disabled={add.isPending}>
						{m.common_cancel()}
					</Button>
					<Button variant="primary" onClick={submit} loading={add.isPending} disabled={name.trim() === ""}>
						{m.passkeys_add_continue()}
					</Button>
				</>
			}
		>
			<form
				onSubmit={(e) => {
					e.preventDefault();
					submit();
				}}
			>
				<Field label={m.passkeys_name_label()} description={m.passkeys_name_hint()} error={error}>
					<Input
						value={name}
						onChange={(e) => setName(e.target.value)}
						maxLength={40}
						// biome-ignore lint/a11y/noAutofocus: the dialog has one field and was opened to fill it
						autoFocus
					/>
				</Field>
			</form>
		</DialogContent>
	);
}

function RemovePasskeyDialog({
	passkey,
	onClose,
	isLast,
}: {
	passkey: Passkey | null;
	onClose: () => void;
	isLast: boolean;
}) {
	const remove = useAction((id: string, api) => api.security.removePasskey(id), {
		success: m.toast_passkey_removed(),
		invalidate: [qk.passkeys, qk.activity],
		onSuccess: onClose,
	});
	return (
		<ConfirmDialog
			open={passkey !== null}
			onOpenChange={(o) => !o && onClose()}
			title={m.passkeys_remove_title()}
			description={
				passkey ? (isLast ? m.passkeys_remove_last({ name: passkey.name }) : m.passkeys_remove_text({ name: passkey.name })) : undefined
			}
			confirmLabel={m.passkeys_remove_confirm()}
			pending={remove.isPending}
			onConfirm={() => passkey && remove.mutate(passkey.id)}
		/>
	);
}

// ---------------------------------------------------------------------------------------------
// E-mail

function EmailSection() {
	const me = useSession().data?.account;
	return (
		<Section title={m.email_title()} flush>
			<Row
				title={
					<span className="flex flex-wrap items-center gap-x-2.5 gap-y-1">
						<Mail aria-hidden strokeWidth={1.75} className="size-4 text-fg-muted" />
						{me ? <span className="break-all">{me.email}</span> : <Skeleton className="h-4 w-48" />}
						<Badge tone="success">{m.email_confirmed()}</Badge>
					</span>
				}
				description={m.email_text()}
			/>
		</Section>
	);
}

// ---------------------------------------------------------------------------------------------
// Devices

function DevicesSection() {
	const devices = useDevices();
	const [revoking, setRevoking] = useState<DeviceSession | null>(null);
	const [everywhere, setEverywhere] = useState(false);
	const others = (devices.data ?? []).filter((d) => !d.current).length;

	return (
		<Section id="devices" title={m.devices_title()} description={m.devices_lead()} flush>
			<QueryView
				query={devices}
				loadingLabel={m.devices_loading()}
				loading={
					<>
						<RowSkeleton />
						<RowSkeleton />
					</>
				}
				errorTitle={m.devices_error()}
				isEmpty={(d) => d.length === 0}
				empty={<EmptyState title={m.devices_empty()} description={m.devices_empty_text()} />}
			>
				{(list) => (
					<>
						<AnimatedRows label={m.devices_title()}>
							{[...list]
								.sort((a, b) => Number(b.current) - Number(a.current))
								.map((d) => (
									<AnimatedRow key={d.id}>
										<div className="flex items-center gap-4 px-5 py-4">
											<IconTile>
												{d.kind === "launcher" ? (
													<Monitor aria-hidden strokeWidth={1.75} />
												) : (
													<Globe aria-hidden strokeWidth={1.75} />
												)}
											</IconTile>
											<div className="min-w-0 flex-1">
												<p className="flex flex-wrap items-center gap-x-2 gap-y-1 text-body font-medium text-fg">
													<span className="truncate">{d.name}</span>
													{d.current ? <Badge tone="accent">{m.devices_this_browser()}</Badge> : null}
													{d.kind === "launcher" ? <Badge>{m.devices_launcher()}</Badge> : null}
												</p>
												<p className="mt-0.5 text-small text-fg-muted">
													{d.platform}
													{d.region ? ` · ${d.region}` : ""}
													{" · "}
													{d.current ? m.devices_active_now() : m.devices_last_active({ when: relativeTime(d.lastUsedAt) })}
												</p>
											</div>
											{d.current ? null : (
												<Button
													variant="ghost"
													size="sm"
													onClick={() => setRevoking(d)}
													aria-label={m.devices_sign_out_named({ name: d.name })}
												>
													{m.devices_sign_out()}
												</Button>
											)}
										</div>
									</AnimatedRow>
								))}
						</AnimatedRows>
						<div className="flex flex-col gap-3 border-t border-line px-5 py-4 sm:flex-row sm:items-center sm:justify-between sm:gap-6">
							<p className="text-body text-fg-muted">{m.devices_everywhere_text()}</p>
							<Button
								variant="danger-soft"
								onClick={() => setEverywhere(true)}
								disabled={others === 0}
								className="shrink-0"
							>
								{m.devices_everywhere()}
							</Button>
						</div>
					</>
				)}
			</QueryView>
			<RevokeDialog device={revoking} onClose={() => setRevoking(null)} />
			<EverywhereDialog open={everywhere} onClose={() => setEverywhere(false)} count={others} />
		</Section>
	);
}

function RevokeDialog({ device, onClose }: { device: DeviceSession | null; onClose: () => void }) {
	const revoke = useAction((id: string, api) => api.security.revokeSession(id), {
		success: m.toast_device_signed_out(),
		invalidate: [qk.devices, qk.activity],
		onSuccess: onClose,
	});
	return (
		<ConfirmDialog
			open={device !== null}
			onOpenChange={(o) => !o && onClose()}
			title={m.devices_revoke_title()}
			description={device ? m.devices_revoke_text({ name: device.name }) : undefined}
			confirmLabel={m.devices_revoke_confirm()}
			pending={revoke.isPending}
			onConfirm={() => device && revoke.mutate(device.id)}
		/>
	);
}

function EverywhereDialog({ open, onClose, count }: { open: boolean; onClose: () => void; count: number }) {
	const qc = useQueryClient();
	const out = useAction((_: void, api) => api.security.signOutEverywhere(), {
		success: m.toast_signed_out_everywhere(),
		invalidate: [qk.devices, qk.activity],
		onSuccess: () => {
			onClose();
			void qc.invalidateQueries({ queryKey: qk.devices });
		},
	});
	return (
		<ConfirmDialog
			open={open}
			onOpenChange={(o) => !o && onClose()}
			title={m.devices_everywhere_title()}
			description={m.devices_everywhere_confirm_text({ count })}
			confirmLabel={m.devices_everywhere_confirm()}
			pending={out.isPending}
			onConfirm={() => out.mutate()}
		/>
	);
}

