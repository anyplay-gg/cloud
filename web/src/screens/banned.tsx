import { useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import { useApi } from "@/api/context";
import { describeError, isApiError } from "@/api/errors";
import { qk, useBan } from "@/api/queries";
import type { Appeal, BanStatement } from "@/api/types";
import { QueryView } from "@/components/query-state";
import { m } from "@/i18n";
import { formatDate } from "@/lib/format";
import { usePageTitle } from "@/lib/use-page-title";
import { Button } from "@/ui/button";
import { Skeleton } from "@/ui/feedback";
import { Field } from "@/ui/field";
import { Textarea } from "@/ui/input";

const MAX = 1000;

/** The statement of a ban and, while it is open, a way to appeal it. */
export function BannedPanel() {
	usePageTitle(m.banned_title());
	const ban = useBan();
	const api = useApi();
	const qc = useQueryClient();
	const navigate = useNavigate();
	const out = async () => {
		await api.auth.signOut().catch(() => undefined);
		qc.clear();
		void navigate({ to: "/sign-in", search: {} });
	};
	return (
		<div className="flex flex-col gap-6">
			<div>
				<h1 className="text-heading font-semibold">{m.banned_title()}</h1>
				<p className="mt-2 text-body text-fg-muted">{m.banned_lead()}</p>
			</div>
			<QueryView
				query={ban}
				loadingLabel={m.banned_loading()}
				loading={
					<div className="space-y-3">
						<Skeleton className="h-4 w-1/2" />
						<Skeleton className="h-16 w-full" />
					</div>
				}
				errorTitle={m.banned_error()}
			>
				{(s) => <Statement statement={s} />}
			</QueryView>
			<Button variant="secondary" onClick={out} className="self-start">
				{m.sign_out()}
			</Button>
		</div>
	);
}

function Statement({ statement }: { statement: BanStatement }) {
	return (
		<div className="flex flex-col gap-6">
			<dl className="divide-y divide-line overflow-hidden rounded-xl bg-surface shadow-card">
				<Item label={m.banned_rule()}>{statement.rule}</Item>
				<Item label={m.banned_reason()}>{statement.reason}</Item>
				<Item label={m.banned_decided()}>{formatDate(statement.decided)}</Item>
				<Item label={m.banned_until()}>
					{statement.until ? formatDate(statement.until) : m.banned_permanent()}
				</Item>
			</dl>
			<AppealBox appeal={statement.appeal} />
		</div>
	);
}

function Item({ label, children }: { label: string; children: React.ReactNode }) {
	return (
		<div className="flex flex-col gap-0.5 px-5 py-3.5 sm:flex-row sm:gap-6">
			<dt className="w-28 shrink-0 text-small font-medium text-fg-muted">{label}</dt>
			<dd className="min-w-0 text-body text-fg">{children}</dd>
		</div>
	);
}

function AppealBox({ appeal }: { appeal: Appeal }) {
	const api = useApi();
	const qc = useQueryClient();
	const [text, setText] = useState("");
	const [error, setError] = useState<string | null>(null);
	const [busy, setBusy] = useState(false);

	if (appeal.state === "over") return <p className="text-body text-fg-muted">{m.appeal_over()}</p>;
	if (appeal.state === "filed") {
		return (
			<div role="status" className="rounded-xl bg-surface p-5 shadow-card">
				<h2 className="text-body font-semibold text-fg">
					{appeal.status === "open"
						? m.appeal_filed_open()
						: appeal.status === "upheld"
							? m.appeal_upheld()
							: m.appeal_reversed()}
				</h2>
				{appeal.answer ? <p className="mt-1.5 text-body text-fg-muted">{appeal.answer}</p> : null}
			</div>
		);
	}
	if (appeal.state === "none") return <p className="text-body text-fg-muted">{m.appeal_none()}</p>;

	const submit = async () => {
		setBusy(true);
		setError(null);
		try {
			await api.auth.appeal(text.trim());
			await qc.invalidateQueries({ queryKey: qk.ban });
		} catch (e) {
			setError(isApiError(e) && e.fields.text ? e.fields.text : describeError(e));
		} finally {
			setBusy(false);
		}
	};

	return (
		<form
			className="flex flex-col gap-4"
			onSubmit={(e) => {
				e.preventDefault();
				if (text.trim()) void submit();
			}}
		>
			<Field
				label={m.appeal_label()}
				description={m.appeal_hint({ date: formatDate(appeal.deadline), left: MAX - text.length })}
				error={error}
			>
				<Textarea rows={5} maxLength={MAX} value={text} onChange={(e) => setText(e.target.value)} />
			</Field>
			<Button
				type="submit"
				variant="primary"
				loading={busy}
				disabled={!text.trim()}
				className="self-start"
			>
				{m.appeal_send()}
			</Button>
		</form>
	);
}
