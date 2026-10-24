import { Download } from "lucide-react";
import { usePageTitle } from "@/lib/use-page-title";
import { useState } from "react";
import { isApiError } from "@/api/errors";
import { qk, useAction, useSession } from "@/api/queries";
import { Page, PageHeader, Section } from "@/components/layout";
import { m } from "@/i18n";
import { daysUntil, formatDate } from "@/lib/format";
import { Button } from "@/ui/button";
import { ConfirmDialog } from "@/ui/dialog";
import { Field } from "@/ui/field";
import { Input } from "@/ui/input";

export function DataPage() {
	usePageTitle(m.nav_data());
	const me = useSession().data?.account;
	return (
		<Page>
			<PageHeader title={m.data_title()} description={m.data_lead()} />
			<ExportSection />
			{me?.deletion ? <PendingDeletion scheduledFor={me.deletion.scheduledFor} /> : <DeleteSection />}
		</Page>
	);
}

function ExportSection() {
	const exporter = useAction(
		async (_: void, api) => {
			const file = await api.data.exportData();
			const url = URL.createObjectURL(file.blob);
			const a = document.createElement("a");
			a.href = url;
			a.download = file.filename;
			document.body.append(a);
			a.click();
			a.remove();
			window.setTimeout(() => URL.revokeObjectURL(url), 4000);
		},
		{ success: m.toast_export_ready() },
	);
	return (
		<Section title={m.data_export_title()} description={m.data_export_lead()}>
			<div className="flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between sm:gap-6">
				<p className="max-w-[52ch] text-body text-fg-muted">{m.data_export_text()}</p>
				<Button
					variant="secondary"
					className="shrink-0"
					onClick={() => exporter.mutate()}
					loading={exporter.isPending}
				>
					{exporter.isPending ? null : <Download aria-hidden />}
					{m.data_export_button()}
				</Button>
			</div>
		</Section>
	);
}

function DeleteSection() {
	const me = useSession().data?.account;
	const [open, setOpen] = useState(false);
	const [typed, setTyped] = useState("");
	const [error, setError] = useState<string | null>(null);
	const del = useAction((handle: string, api) => api.data.requestDeletion({ handle }), {
		success: m.toast_deletion_scheduled(),
		invalidate: [qk.session],
		toastErrors: false,
		onSuccess: () => setOpen(false),
	});
	if (!me) return null;
	const matches = typed.trim().toLowerCase() === me.handle.toLowerCase();
	const thirty = new Date(Date.now() + 30 * 86_400_000).toISOString();

	return (
		<Section
			tone="danger"
			title={m.data_delete_title()}
			description={m.data_delete_lead()}
			className="mt-4"
		>
			<div className="flex flex-col gap-5">
				<div>
					<h3 className="text-body font-medium text-fg">{m.data_delete_list_title()}</h3>
					<ul className="mt-2 list-disc space-y-1 ps-5 text-body text-fg-muted marker:text-fg-faint">
						<li>{m.data_delete_item_1()}</li>
						<li>{m.data_delete_item_2()}</li>
						<li>{m.data_delete_item_3()}</li>
						<li>{m.data_delete_item_4()}</li>
					</ul>
				</div>
				<div className="flex flex-col gap-3 border-t border-line pt-5 sm:flex-row sm:items-center sm:justify-between sm:gap-6">
					<p className="text-body text-fg-muted">{m.data_delete_tip()}</p>
					<Button
						variant="danger-soft"
						className="shrink-0"
						onClick={() => {
							setTyped("");
							setError(null);
							setOpen(true);
						}}
					>
						{m.data_delete_button()}
					</Button>
				</div>
			</div>
			<ConfirmDialog
				open={open}
				onOpenChange={setOpen}
				title={m.data_delete_confirm_title()}
				description={m.data_delete_confirm_text({ date: formatDate(thirty) })}
				confirmLabel={m.data_delete_confirm_button()}
				pending={del.isPending}
				confirmDisabled={!matches}
				onConfirm={() =>
					del.mutate(typed, {
						onError: (e) => {
							if (isApiError(e) && e.fields.handle) setError(e.fields.handle);
						},
					})
				}
			>
				<form
					onSubmit={(e) => {
						e.preventDefault();
						if (matches && !del.isPending) del.mutate(typed);
					}}
				>
					<Field label={m.data_delete_confirm_label({ handle: `@${me.handle}` })} error={error}>
						<Input
							value={typed}
							onChange={(e) => setTyped(e.target.value)}
							autoComplete="off"
							autoCapitalize="none"
							spellCheck={false}
							className="font-mono"
						/>
					</Field>
				</form>
			</ConfirmDialog>
		</Section>
	);
}

function PendingDeletion({ scheduledFor }: { scheduledFor: string }) {
	const cancel = useAction((_: void, api) => api.data.cancelDeletion(), {
		success: m.toast_deletion_cancelled(),
		invalidate: [qk.session],
	});
	return (
		<Section tone="danger" title={m.data_pending_title({ date: formatDate(scheduledFor) })} className="mt-4">
			<div className="flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between sm:gap-6">
				<p className="max-w-[52ch] text-body text-fg-muted">
					{m.data_pending_text()} {m.deletion_banner_body({ days: daysUntil(scheduledFor) })}
				</p>
				<Button variant="primary" className="shrink-0" onClick={() => cancel.mutate()} loading={cancel.isPending}>
					{m.deletion_cancel()}
				</Button>
			</div>
		</Section>
	);
}
