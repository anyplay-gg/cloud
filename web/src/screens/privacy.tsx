import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { useApi } from "@/api/context";
import { qk, usePrivacy } from "@/api/queries";
import type { Audience, Privacy as PrivacySettings, ProfilePart } from "@/api/types";
import { Page, PageHeader, Row, Section } from "@/components/layout";
import { QueryView } from "@/components/query-state";
import { m } from "@/i18n";
import { usePageTitle } from "@/lib/use-page-title";
import { useSaveState } from "@/lib/use-save-state";
import { Skeleton } from "@/ui/feedback";
import { SaveStatus } from "@/ui/save-status";
import { Select } from "@/ui/select";
import { toast } from "@/ui/toast";

const PARTS: { part: ProfilePart; title: () => string; text: () => string }[] = [
	{ part: "about", title: () => m.privacy_field_about(), text: () => m.privacy_field_about_text() },
	{
		part: "favourites",
		title: () => m.privacy_field_favourites(),
		text: () => m.privacy_field_favourites_text(),
	},
	{
		part: "avatar",
		title: () => m.privacy_field_avatar(),
		text: () => m.privacy_field_avatar_text(),
	},
	{
		part: "playtime",
		title: () => m.privacy_field_playtime(),
		text: () => m.privacy_field_playtime_text(),
	},
	{
		part: "trophies",
		title: () => m.privacy_field_trophies(),
		text: () => m.privacy_field_trophies_text(),
	},
];

const audienceOptions = (): { value: Audience; label: string; description: string }[] => [
	{
		value: "friends",
		label: m.privacy_audience_friends(),
		description: m.privacy_audience_friends_text(),
	},
	{
		value: "everyone",
		label: m.privacy_audience_everyone(),
		description: m.privacy_audience_everyone_text(),
	},
	{
		value: "nobody",
		label: m.privacy_audience_nobody(),
		description: m.privacy_audience_nobody_text(),
	},
];

const SKELETON_ROWS = ["a", "b", "c", "d", "e"];

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
					<div className="overflow-hidden rounded-xl bg-surface shadow-card">
						{SKELETON_ROWS.map((id) => (
							<div
								key={id}
								className="flex items-center justify-between gap-6 border-t border-line px-5 py-4 first:border-t-0"
							>
								<div className="space-y-2">
									<Skeleton className="h-4 w-32" />
									<Skeleton className="h-3 w-56" />
								</div>
								<Skeleton className="h-9 w-36" />
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
			const stored = await api.account.updatePrivacy(next);
			qc.setQueryData(qk.privacy, stored);
			save.saved();
		} catch {
			setSettings(previous);
			save.fail();
			toast.error(m.privacy_save_failed());
		}
	};

	return (
		<Section
			id="profile-visibility"
			title={m.privacy_profile_title()}
			description={m.privacy_profile_lead()}
			flush
			actions={<SaveStatus state={save.state} />}
		>
			<ul className="divide-y divide-line">
				{PARTS.map((f) => (
					<Row
						as="li"
						key={f.part}
						title={<label htmlFor={`aud-${f.part}`}>{f.title()}</label>}
						description={f.text()}
						control={
							<Select
								id={`aud-${f.part}`}
								className="w-44"
								value={settings[f.part]}
								options={audienceOptions()}
								onValueChange={(v) => void commit({ ...settings, [f.part]: v })}
							/>
						}
					/>
				))}
			</ul>
		</Section>
	);
}
