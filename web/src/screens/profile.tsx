import { useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { ArrowUpRight, ImageUp } from "lucide-react";
import { useRef, useState } from "react";
import { useForm } from "react-hook-form";
import { useApi } from "@/api/context";
import { describeError, isApiError } from "@/api/errors";
import { qk, useAction, useOwnProfile, useSession, useTrophies } from "@/api/queries";
import type { Profile } from "@/api/types";
import { Page, PageHeader, Section } from "@/components/layout";
import { ProfileBanner } from "@/components/profile-banner";
import { QueryView } from "@/components/query-state";
import { m } from "@/i18n";
import { formatPlaytime } from "@/lib/format";
import { accentFor, avatarUrl, themeFor } from "@/lib/identity";
import { usePageTitle } from "@/lib/use-page-title";
import { useSaveState } from "@/lib/use-save-state";
import { Avatar } from "@/ui/avatar";
import { Button, buttonClass } from "@/ui/button";
import { CopyField } from "@/ui/copy-field";
import { Skeleton } from "@/ui/feedback";
import { Field } from "@/ui/field";
import { Input, Textarea } from "@/ui/input";
import { SaveStatus } from "@/ui/save-status";
import { toast } from "@/ui/toast";
import { Checkbox } from "@/ui/toggle";

const ABOUT_MAX = 160;
const FAVOURITES_MAX = 3;
const AVATAR_MAX = 1_000_000;

export function ProfileEdit() {
	usePageTitle(m.nav_profile());
	const me = useSession().data?.account;
	const profile = useOwnProfile();
	return (
		<Page>
			<PageHeader
				title={m.profile_title()}
				description={m.profile_lead()}
				actions={
					me ? (
						<Link
							to="/u/$name"
							params={{ name: me.handle }}
							className={buttonClass({ variant: "ghost" })}
						>
							{m.menu_view_profile()}
							<ArrowUpRight aria-hidden />
						</Link>
					) : null
				}
			/>
			<QueryView
				query={profile}
				loadingLabel={m.profile_loading()}
				errorTitle={m.profile_error()}
				loading={
					<div className="flex flex-col gap-10">
						<Skeleton className="h-48 w-full rounded-xl" />
						<Skeleton className="h-56 w-full rounded-xl" />
					</div>
				}
			>
				{(p) => <ProfileForm profile={p} />}
			</QueryView>
		</Page>
	);
}

function ProfileForm({ profile }: { profile: Profile }) {
	const api = useApi();
	const qc = useQueryClient();
	const save = useSaveState();
	const trophies = useTrophies();
	const handle = profile.account.handle;
	const [favourites, setFavourites] = useState<string[]>(profile.favourites ?? []);
	const {
		register,
		handleSubmit,
		watch,
		reset,
		setError,
		formState: { errors, isDirty, isSubmitting },
	} = useForm<{ about: string }>({ defaultValues: { about: profile.about ?? "" } });
	const about = watch("about");
	const favouritesDirty =
		favourites.length !== (profile.favourites ?? []).length ||
		favourites.some((f, i) => f !== (profile.favourites ?? [])[i]);
	const dirty = isDirty || favouritesDirty;

	// Titles the player can pick: the ones that have trophies or playtime. Never names.
	const known = new Set<string>([
		...(trophies.data?.titles.map((t) => t.titleId) ?? []),
		...(profile.playtime?.map((p) => p.titleId) ?? []),
		...favourites,
	]);

	const submit = handleSubmit(async (values) => {
		save.saving();
		try {
			const next = await api.account.updateProfile({ about: values.about.trim(), favourites });
			qc.setQueryData(qk.profile, next);
			reset({ about: next.about ?? "" });
			setFavourites(next.favourites ?? []);
			save.saved();
		} catch (e) {
			save.fail();
			if (isApiError(e) && e.fields.about) setError("about", { message: e.fields.about });
			else toast.error(describeError(e));
		}
	});

	return (
		<form onSubmit={submit} noValidate className="flex flex-col gap-10">
			<Section title={m.profile_preview()} description={m.profile_preview_lead()} flush>
				<ProfileBanner theme={themeFor(handle)} className="h-24" />
				<div className="flex items-start gap-4 px-5 pb-5">
					<Avatar
						name={handle}
						accent={accentFor(handle)}
						src={avatarUrl(profile.avatar)}
						size="lg"
						className="-mt-8 rounded-full ring-4 ring-surface"
					/>
					<div className="min-w-0 pt-2.5">
						<p className="truncate font-mono text-lead font-semibold text-fg">@{handle}</p>
						{about.trim() ? (
							<p className="mt-2 max-w-[56ch] text-body text-fg">{about.trim()}</p>
						) : (
							<p className="mt-2 text-body text-fg-muted">{m.overview_no_about()}</p>
						)}
					</div>
				</div>
			</Section>

			<AvatarSection profile={profile} />

			<Section title={m.profile_details()} description={m.profile_details_lead()}>
				<div className="flex flex-col gap-5">
					<Field
						label={m.profile_about_label()}
						description={m.profile_about_hint({ count: ABOUT_MAX - about.length })}
						error={errors.about?.message}
					>
						<Textarea rows={3} maxLength={ABOUT_MAX} {...register("about")} />
					</Field>
					<Field label={m.auth_handle_label()} description={m.profile_handle_hint()}>
						<Input value={`@${handle}`} readOnly className="font-mono" />
					</Field>
					<Field label={m.profile_link_label()} description={m.profile_link_hint()}>
						<CopyField
							value={`${window.location.origin}/u/${handle}`}
							label={m.profile_link_label()}
						/>
					</Field>
				</div>
			</Section>

			<Section
				title={m.profile_favourites()}
				description={m.profile_favourites_lead({ max: FAVOURITES_MAX })}
			>
				{known.size === 0 ? (
					<p className="text-body text-fg-muted">{m.profile_favourites_none()}</p>
				) : (
					<ul className="grid gap-2 sm:grid-cols-2">
						{[...known].sort().map((id) => {
							const checked = favourites.includes(id);
							const full = favourites.length >= FAVOURITES_MAX && !checked;
							return (
								<li key={id}>
									{/* biome-ignore lint/a11y/noLabelWithoutControl: the Base UI checkbox inside is the control */}
									<label
										className={`flex items-center gap-3 rounded-lg px-3 py-2.5 text-body shadow-[inset_0_0_0_1px_var(--line-strong)] ${full ? "opacity-50" : "pointer-hover:hover:bg-hover"}`}
									>
										<Checkbox
											checked={checked}
											disabled={full}
											onCheckedChange={(on) =>
												setFavourites((f) => (on ? [...f, id] : f.filter((x) => x !== id)))
											}
										/>
										<span className="font-mono text-small">{id}</span>
									</label>
								</li>
							);
						})}
					</ul>
				)}
			</Section>

			{profile.playtime && profile.playtime.length > 0 ? (
				<Section title={m.profile_playtime()} description={m.profile_playtime_lead()} flush>
					<ul className="divide-y divide-line">
						{profile.playtime.map((p) => (
							<li key={p.titleId} className="flex items-center justify-between gap-4 px-5 py-3.5">
								<span className="font-mono text-small text-fg">{p.titleId}</span>
								<span className="nums text-body text-fg-muted">{formatPlaytime(p.minutes)}</span>
							</li>
						))}
					</ul>
				</Section>
			) : null}

			<div className="sticky bottom-0 -mx-4 flex items-center justify-between gap-4 border-t border-line bg-page/90 px-4 py-3 backdrop-blur-md sm:-mx-0 sm:rounded-xl sm:border sm:px-5">
				<SaveStatus state={save.state} />
				<div className="flex items-center gap-3">
					<Button
						type="button"
						variant="ghost"
						disabled={!dirty || isSubmitting}
						onClick={() => {
							reset();
							setFavourites(profile.favourites ?? []);
						}}
					>
						{m.profile_discard()}
					</Button>
					<Button type="submit" variant="primary" loading={isSubmitting} disabled={!dirty}>
						{m.common_save()}
					</Button>
				</div>
			</div>
		</form>
	);
}

function AvatarSection({ profile }: { profile: Profile }) {
	const input = useRef<HTMLInputElement>(null);
	const upload = useAction((file: File, api) => api.account.setAvatar(file), {
		success: m.toast_avatar_saved(),
		invalidate: [qk.profile],
	});
	const remove = useAction((_: undefined, api) => api.account.removeAvatar(), {
		success: m.toast_avatar_removed(),
		invalidate: [qk.profile],
	});
	const pick = (file: File | undefined) => {
		if (!file) return;
		if (file.type !== "image/png" && file.type !== "image/jpeg") {
			toast.error(m.avatar_type());
			return;
		}
		if (file.size > AVATAR_MAX) {
			toast.error(m.avatar_too_large());
			return;
		}
		upload.mutate(file);
	};
	return (
		<Section title={m.avatar_title()} description={m.avatar_lead()}>
			<div className="flex flex-col gap-4 sm:flex-row sm:items-center">
				<Avatar
					name={profile.account.handle}
					accent={accentFor(profile.account.handle)}
					src={avatarUrl(profile.avatar)}
					size="xl"
				/>
				<div className="flex flex-wrap items-center gap-3">
					<input
						ref={input}
						type="file"
						accept="image/png,image/jpeg"
						className="sr-only"
						tabIndex={-1}
						aria-hidden
						onChange={(e) => {
							pick(e.target.files?.[0]);
							e.target.value = "";
						}}
					/>
					<Button
						variant="secondary"
						onClick={() => input.current?.click()}
						loading={upload.isPending}
					>
						{upload.isPending ? null : <ImageUp aria-hidden />}
						{profile.avatar ? m.avatar_replace() : m.avatar_upload()}
					</Button>
					{profile.avatar ? (
						<Button
							variant="ghost"
							onClick={() => remove.mutate(undefined)}
							loading={remove.isPending}
						>
							{m.avatar_remove()}
						</Button>
					) : null}
				</div>
			</div>
			<p className="mt-3 text-small text-fg-muted">{m.avatar_hint()}</p>
		</Section>
	);
}
