import { useQueryClient } from "@tanstack/react-query";
import { usePageTitle } from "@/lib/use-page-title";
import { useEffect, useState } from "react";
import { useApi } from "@/api/context";
import { qk, usePrivacy } from "@/api/queries";
import type { Audience, FriendRequestPolicy, PrivacySettings, ProfileField } from "@/api/types";
import { Page, PageHeader, Row, Section } from "@/components/layout";
import { QueryView } from "@/components/query-state";
import { m } from "@/i18n";
import { useSaveState } from "@/lib/use-save-state";
import { Skeleton } from "@/ui/feedback";
import { SaveStatus } from "@/ui/save-status";
import { Select } from "@/ui/select";
import { toast } from "@/ui/toast";

const FIELDS: { field: ProfileField; title: () => string; text: () => string }[] = [
	{ field: "about", title: () => m.privacy_field_about(), text: () => m.privacy_field_about_text() },
	{ field: "friends", title: () => m.privacy_field_friends(), text: () => m.privacy_field_friends_text() },
	{ field: "trophies", title: () => m.privacy_field_trophies(), text: () => m.privacy_field_trophies_text() },
	{ field: "activity", title: () => m.privacy_field_activity(), text: () => m.privacy_field_activity_text() },
];

const audienceOptions = (): { value: Audience; label: string; description: string }[] => [
	{ value: "everyone", label: m.privacy_audience_everyone(), description: m.privacy_audience_everyone_text() },
	{ value: "friends", label: m.privacy_audience_friends(), description: m.privacy_audience_friends_text() },
	{ value: "me", label: m.privacy_audience_me(), description: m.privacy_audience_me_text() },
];

const requestOptions = (): { value: FriendRequestPolicy; label: string; description: string }[] => [
	{ value: "everyone", label: m.privacy_requests_everyone(), description: m.privacy_requests_everyone_text() },
	{ value: "friends_of_friends", label: m.privacy_requests_fof(), description: m.privacy_requests_fof_text() },
	{ value: "nobody", label: m.privacy_requests_nobody(), description: m.privacy_requests_nobody_text() },
];

export function Privacy() {
	usePageTitle(m.nav_privacy());
	const privacy = usePrivacy();
	return (
		<Page>
			<PageHeader title={m.privacy_title()} description={m.privacy_lead()} />
			<QueryView
				query={privacy}
				loadingLabel={m.privacy_loading()}
				errorTitle={m.privacy_error()}
				loading={
					<div className="flex flex-col gap-8">
						{[4, 1].map((rows) => (
							<div key={rows} className="overflow-hidden rounded-xl bg-surface shadow-card">
								{Array.from({ length: rows }, (_, i) => (
									<div key={`r${i}`} className="flex items-center justify-between gap-6 border-t border-line px-5 py-4 first:border-t-0">
										<div className="space-y-2">
											<Skeleton className="h-4 w-32" />
											<Skeleton className="h-3 w-56" />
										</div>
										<Skeleton className="h-9 w-36" />
									</div>
								))}
							</div>
						))}
					</div>
				}
			>
				{(settings) => <PrivacyForm initial={settings} />}
			</QueryView>
		</Page>
	);
}

function PrivacyForm({ initial }: { initial: PrivacySettings }) {
	const api = useApi();
	const qc = useQueryClient();
	const [settings, setSettings] = useState(initial);
	const save = useSaveState();

	// a refetch from elsewhere (another tab) replaces the local copy
	useEffect(() => setSettings(initial), [initial]);

	const commit = async (next: PrivacySettings) => {
		const previous = settings;
		setSettings(next);
		save.saving();
		try {
			const stored = await api.privacy.update(next);
			qc.setQueryData(qk.privacy, stored);
			save.saved();
		} catch {
			setSettings(previous);
			save.fail();
			toast.error(m.privacy_save_failed());
		}
	};

	return (
		<>
			<Section
				id="profile-visibility"
				title={m.privacy_profile_title()}
				description={m.privacy_profile_lead()}
				flush
				actions={<SaveStatus state={save.state} />}
			>
				<ul className="divide-y divide-line">
					{FIELDS.map((f) => (
						<Row
							as="li"
							key={f.field}
							title={<label htmlFor={`aud-${f.field}`}>{f.title()}</label>}
							description={f.text()}
							control={
								<Select
									id={`aud-${f.field}`}
									className="w-44"
									value={settings.fields[f.field]}
									options={audienceOptions()}
									onValueChange={(v) =>
										void commit({ ...settings, fields: { ...settings.fields, [f.field]: v } })
									}
								/>
							}
						/>
					))}
				</ul>
			</Section>
			<Section id="friend-requests" title={m.privacy_requests_title()} description={m.privacy_requests_lead()} flush>
				<Row
					title={<label htmlFor="friend-requests-select">{m.privacy_requests_label()}</label>}
					control={
						<Select
							id="friend-requests-select"
							className="w-52"
							value={settings.friendRequests}
							options={requestOptions()}
							onValueChange={(v) => void commit({ ...settings, friendRequests: v })}
						/>
					}
				/>
			</Section>
		</>
	);
}
