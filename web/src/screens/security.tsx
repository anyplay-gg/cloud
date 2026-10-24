import { useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { Globe, KeyRound, Mail, Monitor, Pencil, Plus } from "lucide-react";
import { useState } from "react";
import { useApi } from "@/api/context";
import { describeError, isApiError } from "@/api/errors";
import { qk, useAccountView, useAction, useSession } from "@/api/queries";
import type { BrowserSession, Device, Passkey } from "@/api/types";
import { AnimatedRow, AnimatedRows } from "@/components/animated-rows";
import { Page, PageHeader, Section } from "@/components/layout";
import { QueryView } from "@/components/query-state";
import { isStepUpCancelled, useStepUp } from "@/features/step-up";
import { m } from "@/i18n";
import { formatDate, relativeTime } from "@/lib/format";
import { usePageTitle } from "@/lib/use-page-title";
import { useUndoable } from "@/lib/use-undoable";
import { PresenceDot } from "@/ui/avatar";
import { Badge } from "@/ui/badge";
import { Button } from "@/ui/button";
import { ConfirmMorph } from "@/ui/confirm-morph";
import { ConfirmDialog, Dialog, DialogContent } from "@/ui/dialog";
import { EmptyState, Skeleton } from "@/ui/feedback";
import { Field } from "@/ui/field";
import { Input } from "@/ui/input";
import { OtpInput } from "@/ui/otp-input";
import { toast } from "@/ui/toast";

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
	const account = useAccountView();
	const [adding, setAdding] = useState(false);
	const [renaming, setRenaming] = useState<Passkey | null>(null);
	const count = account.data?.passkeys.length ?? 0;
	const max = account.data?.maxPasskeys ?? 10;
	const canAdd = !account.isError && count < max;

	return (
		<Section
			id="passkeys"
			title={m.passkeys_title()}
			description={m.passkeys_lead({ max })}
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
				query={account}
				loadingLabel={m.passkeys_loading()}
				loading={<RowSkeleton />}
				errorTitle={m.passkeys_error()}
				isEmpty={(d) => d.passkeys.length === 0}
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
				{(a) => (
					<AnimatedRows label={m.passkeys_title()}>
						{a.passkeys.map((k) => (
							<AnimatedRow key={k.id}>
								<div className="flex items-center gap-3 px-5 py-4 sm:gap-4">
									<IconTile>
										<KeyRound aria-hidden strokeWidth={1.75} />
									</IconTile>
									<div className="min-w-0 flex-1">
										<p className="truncate text-body font-medium text-fg">{k.name}</p>
										<p className="mt-0.5 text-small text-fg-muted">
											{m.passkeys_added({ date: formatDate(k.created, "medium") })}
											{" · "}
											{k.lastUsed
												? m.passkeys_last_used({ when: relativeTime(k.lastUsed) })
												: m.passkeys_never_used()}
										</p>
									</div>
									<Button
										variant="ghost"
										size="sm"
										icon
										onClick={() => setRenaming(k)}
										aria-label={m.passkeys_rename_named({ name: k.name })}
									>
										<Pencil aria-hidden />
									</Button>
									<PasskeyRemove passkey={k} isLast={a.passkeys.length === 1} />
								</div>
							</AnimatedRow>
						))}
					</AnimatedRows>
				)}
			</QueryView>
			<AddPasskeyDialog open={adding} onOpenChange={setAdding} />
			<RenamePasskeyDialog passkey={renaming} onClose={() => setRenaming(null)} />
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

function AddPasskeyDialog({
	open,
	onOpenChange,
}: {
	open: boolean;
	onOpenChange: (o: boolean) => void;
}) {
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
		invalidate: [qk.account],
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
					<Button
						variant="primary"
						onClick={submit}
						loading={add.isPending}
						disabled={name.trim() === ""}
					>
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
					<Input value={name} onChange={(e) => setName(e.target.value)} maxLength={40} autoFocus />
				</Field>
			</form>
		</DialogContent>
	);
}

function RenamePasskeyDialog({
	passkey,
	onClose,
}: {
	passkey: Passkey | null;
	onClose: () => void;
}) {
	return (
		<Dialog open={passkey !== null} onOpenChange={(o) => !o && onClose()}>
			{passkey ? <RenameBody passkey={passkey} onDone={onClose} /> : null}
		</Dialog>
	);
}

function RenameBody({ passkey, onDone }: { passkey: Passkey; onDone: () => void }) {
	const [name, setName] = useState(passkey.name);
	const rename = useAction((n: string, api) => api.security.renamePasskey(passkey.id, n), {
		success: m.toast_passkey_renamed(),
		invalidate: [qk.account],
		onSuccess: onDone,
	});
	const submit = () => name.trim() && rename.mutate(name.trim());
	return (
		<DialogContent
			title={m.passkeys_rename_title()}
			footer={
				<>
					<Button variant="secondary" onClick={onDone} disabled={rename.isPending}>
						{m.common_cancel()}
					</Button>
					<Button
						variant="primary"
						onClick={submit}
						loading={rename.isPending}
						disabled={name.trim() === "" || name.trim() === passkey.name}
					>
						{m.common_save()}
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
				<Field label={m.passkeys_name_label()}>
					<Input value={name} onChange={(e) => setName(e.target.value)} maxLength={40} autoFocus />
				</Field>
			</form>
		</DialogContent>
	);
}

function PasskeyRemove({ passkey, isLast }: { passkey: Passkey; isLast: boolean }) {
	const remove = useAction((id: string, api) => api.security.removePasskey(id), {
		success: m.toast_passkey_removed(),
		invalidate: [qk.account],
	});
	return (
		<ConfirmMorph
			label={m.common_remove()}
			confirmLabel={isLast ? m.passkeys_remove_last_confirm() : m.passkeys_remove_confirm()}
			ariaLabel={m.passkeys_remove_named({ name: passkey.name })}
			confirmAriaLabel={m.passkeys_confirm_named({ name: passkey.name })}
			description={
				isLast
					? m.passkeys_remove_last({ name: passkey.name })
					: m.passkeys_remove_text({ name: passkey.name })
			}
			pending={remove.isPending}
			onConfirm={() => remove.mutate(passkey.id)}
		/>
	);
}

// ---------------------------------------------------------------------------------------------
// E-mail

function EmailSection() {
	const me = useSession().data?.account;
	const account = useAccountView();
	const [open, setOpen] = useState(false);
	const pending = account.data?.pendingEmail ?? null;
	return (
		<Section title={m.email_title()} flush>
			<div className="flex flex-col gap-3 px-5 py-4 sm:flex-row sm:items-center sm:justify-between sm:gap-6">
				<div className="min-w-0">
					<p className="flex flex-wrap items-center gap-x-2.5 gap-y-1 text-body font-medium text-fg">
						<Mail aria-hidden strokeWidth={1.75} className="size-4 text-fg-muted" />
						{me ? <span className="break-all">{me.email}</span> : <Skeleton className="h-4 w-48" />}
						<Badge tone="success">{m.email_confirmed()}</Badge>
					</p>
					<p className="mt-1 text-body text-fg-muted">
						{pending ? m.email_pending({ email: pending }) : m.email_text()}
					</p>
				</div>
				<Button variant="secondary" className="shrink-0" onClick={() => setOpen(true)}>
					{pending ? m.email_enter_code() : m.email_change()}
				</Button>
			</div>
			<Dialog open={open} onOpenChange={setOpen}>
				{open ? <ChangeEmailBody pending={pending} onDone={() => setOpen(false)} /> : null}
			</Dialog>
		</Section>
	);
}

function ChangeEmailBody({ pending, onDone }: { pending: string | null; onDone: () => void }) {
	const api = useApi();
	const qc = useQueryClient();
	const stepUp = useStepUp();
	const [email, setEmail] = useState("");
	const [sentTo, setSentTo] = useState<string | null>(pending);
	const [code, setCode] = useState("");
	const [error, setError] = useState<string | null>(null);
	const [busy, setBusy] = useState(false);

	const send = async () => {
		setBusy(true);
		setError(null);
		try {
			await stepUp.run(() => api.account.changeEmail(email.trim()));
			setSentTo(email.trim());
			void qc.invalidateQueries({ queryKey: qk.account });
		} catch (e) {
			if (!isStepUpCancelled(e))
				setError(isApiError(e) && e.fields.email ? e.fields.email : describeError(e));
		} finally {
			setBusy(false);
		}
	};
	const confirm = async (value: string) => {
		if (value.length < 6 || busy) return;
		setBusy(true);
		setError(null);
		try {
			const confirmed = await stepUp.run(() => api.account.confirmEmail(value));
			toast.success(m.toast_email_changed());
			await qc.invalidateQueries({ queryKey: qk.session });
			void qc.invalidateQueries({ queryKey: qk.account });
			void confirmed;
			onDone();
		} catch (e) {
			setCode("");
			if (!isStepUpCancelled(e)) setError(describeError(e));
		} finally {
			setBusy(false);
		}
	};

	return sentTo ? (
		<DialogContent title={m.email_code_title()} description={m.email_code_text({ email: sentTo })}>
			<form
				className="flex flex-col gap-5"
				onSubmit={(e) => {
					e.preventDefault();
					void confirm(code);
				}}
			>
				<Field label={m.auth_code_label()} error={error}>
					<OtpInput
						value={code}
						onChange={setCode}
						onComplete={(v) => void confirm(v)}
						invalid={Boolean(error)}
						disabled={busy}
						autoFocus
					/>
				</Field>
				<div className="flex flex-col-reverse gap-3 sm:flex-row sm:justify-between">
					<Button type="button" variant="ghost" onClick={() => setSentTo(null)}>
						{m.email_other()}
					</Button>
					<Button type="submit" variant="primary" loading={busy} disabled={code.length < 6}>
						{m.email_confirm()}
					</Button>
				</div>
			</form>
		</DialogContent>
	) : (
		<DialogContent title={m.email_change_title()} description={m.email_change_text()}>
			<form
				className="flex flex-col gap-5"
				onSubmit={(e) => {
					e.preventDefault();
					if (email.trim()) void send();
				}}
			>
				<Field label={m.email_new_label()} error={error}>
					<Input
						type="email"
						inputMode="email"
						autoComplete="email"
						autoCapitalize="none"
						spellCheck={false}
						value={email}
						onChange={(e) => setEmail(e.target.value)}
						placeholder={m.auth_email_placeholder()}
						autoFocus
					/>
				</Field>
				<div className="flex flex-col-reverse gap-3 sm:flex-row sm:justify-end">
					<Button type="button" variant="secondary" onClick={onDone}>
						{m.common_cancel()}
					</Button>
					<Button type="submit" variant="primary" loading={busy} disabled={!email.trim()}>
						{m.auth_send_code()}
					</Button>
				</div>
			</form>
		</DialogContent>
	);
}

// ---------------------------------------------------------------------------------------------
// Launchers and browsers

/**
 * Active sessions, the way ObsidianUI lays them out (pattern, written again): the browser you are
 * in first and set apart, then everything else that can open the account. Signing a launcher or
 * another browser out is held back for six seconds with an Undo, because it is cheap to hold
 * and easy to misclick.
 */
function DevicesSection() {
	const api = useApi();
	const qc = useQueryClient();
	const stepUp = useStepUp();
	const account = useAccountView();
	const [everywhere, setEverywhere] = useState(false);

	type Target = { key: string; name: string; run: () => Promise<unknown> };
	const undoable = useUndoable<Target>(
		async (t) => {
			try {
				await stepUp.run(() => t.run());
				toast.success(m.toast_device_signed_out());
			} catch (e) {
				if (!isStepUpCancelled(e)) toast.error(describeError(e));
			} finally {
				void qc.invalidateQueries({ queryKey: qk.account });
			}
		},
		(t) => t.key,
	);

	const data = account.data;
	const launchers = (data?.devices ?? []).filter((d) => !undoable.hidden.has(`d${d.id}`));
	const browsers = (data?.sessions ?? []).filter((s) => !undoable.hidden.has(`s${s.id}`));
	const current = browsers.find((s) => s.current);
	const otherBrowsers = browsers.filter((s) => !s.current);
	const others = launchers.length + otherBrowsers.length;

	const signOutLauncher = (d: Device) =>
		undoable.run(
			{ key: `d${d.id}`, name: d.name, run: () => api.security.removeDevice(d.id) },
			m.toast_device_signed_out_undo({ name: d.name }),
		);
	const signOutBrowser = (s: BrowserSession) =>
		undoable.run(
			{ key: `s${s.id}`, name: m.devices_browser(), run: () => api.security.revokeSession(s.id) },
			m.toast_device_signed_out_undo({ name: m.devices_browser() }),
		);

	return (
		<Section id="devices" title={m.devices_title()} description={m.devices_lead()} flush>
			<QueryView
				query={account}
				loadingLabel={m.devices_loading()}
				loading={
					<>
						<RowSkeleton />
						<RowSkeleton />
					</>
				}
				errorTitle={m.devices_error()}
			>
				{() => (
					<>
						{current ? (
							<div className="flex items-center gap-4 bg-accent-soft/50 px-5 py-4">
								<IconTile>
									<Globe aria-hidden strokeWidth={1.75} />
								</IconTile>
								<div className="min-w-0 flex-1">
									<p className="flex flex-wrap items-center gap-x-2 gap-y-1 text-body font-medium text-fg">
										<span className="truncate">{m.devices_this_browser()}</span>
										<Badge tone="accent">{m.devices_current()}</Badge>
									</p>
									<p className="mt-0.5 flex items-center gap-2 text-small text-fg-muted">
										<PresenceDot status="online" />
										{m.devices_signed_in_since({ date: formatDate(current.created, "medium") })}
										{" · "}
										{m.devices_active_now()}
									</p>
								</div>
							</div>
						) : null}
						<div className="border-t border-line px-5 pb-1 pt-4">
							<h3 className="text-small font-medium text-fg-muted">{m.security_others()}</h3>
						</div>
						{others === 0 ? (
							<p className="px-5 pb-4 pt-1 text-body text-fg-muted">{m.devices_empty_text()}</p>
						) : (
							<AnimatedRows label={m.security_others()}>
								{launchers.map((d) => (
									<AnimatedRow key={`d${d.id}`}>
										<div className="flex items-center gap-4 px-5 py-4">
											<IconTile>
												<Monitor aria-hidden strokeWidth={1.75} />
											</IconTile>
											<div className="min-w-0 flex-1">
												<p className="flex flex-wrap items-center gap-x-2 gap-y-1 text-body font-medium text-fg">
													<span className="truncate">{d.name}</span>
													<Badge>{m.devices_launcher()}</Badge>
												</p>
												<p className="mt-0.5 text-small text-fg-muted">
													{platformName(d.platform)}
													{" · "}
													{m.devices_last_active({ when: relativeTime(d.lastUsed) })}
												</p>
											</div>
											<Button
												variant="ghost"
												size="sm"
												onClick={() => signOutLauncher(d)}
												aria-label={m.devices_sign_out_named({ name: d.name })}
											>
												{m.devices_sign_out()}
											</Button>
										</div>
									</AnimatedRow>
								))}
								{otherBrowsers.map((s) => (
									<AnimatedRow key={`s${s.id}`}>
										<div className="flex items-center gap-4 px-5 py-4">
											<IconTile>
												<Globe aria-hidden strokeWidth={1.75} />
											</IconTile>
											<div className="min-w-0 flex-1">
												<p className="truncate text-body font-medium text-fg">
													{m.devices_browser()}
												</p>
												<p className="mt-0.5 text-small text-fg-muted">
													{m.devices_signed_in_since({ date: formatDate(s.created, "medium") })}
													{" · "}
													{m.devices_last_active({ when: relativeTime(s.lastUsed) })}
												</p>
											</div>
											<Button
												variant="ghost"
												size="sm"
												onClick={() => signOutBrowser(s)}
												aria-label={m.devices_sign_out_browser_named({
													date: formatDate(s.created, "medium"),
												})}
											>
												{m.devices_sign_out()}
											</Button>
										</div>
									</AnimatedRow>
								))}
							</AnimatedRows>
						)}
						<div className="flex flex-col gap-3 border-t border-line px-5 py-4 sm:flex-row sm:items-center sm:justify-between sm:gap-6">
							<p className="text-body text-fg-muted">{m.devices_everywhere_text()}</p>
							<Button
								variant="danger-soft"
								onClick={() => setEverywhere(true)}
								className="shrink-0"
							>
								{m.devices_everywhere()}
							</Button>
						</div>
					</>
				)}
			</QueryView>
			<EverywhereDialog open={everywhere} onClose={() => setEverywhere(false)} count={others} />
		</Section>
	);
}

const platformName = (p: string) =>
	p === "windows"
		? "Windows"
		: p === "linux"
			? "Linux"
			: p === "macos"
				? "macOS"
				: m.devices_platform_other();

function EverywhereDialog({
	open,
	onClose,
	count,
}: {
	open: boolean;
	onClose: () => void;
	count: number;
}) {
	const navigate = useNavigate();
	const qc = useQueryClient();
	const out = useAction((_: undefined, api) => api.security.signOutEverywhere(), {
		success: m.toast_signed_out_everywhere(),
		onSuccess: () => {
			onClose();
			// this browser is signed out too: drop what it knew and let the guards send it to sign in
			qc.clear();
			void navigate({ to: "/sign-in", search: {} });
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
			onConfirm={() => out.mutate(undefined)}
		/>
	);
}
