import { zodResolver } from "@hookform/resolvers/zod";
import { usePageTitle } from "@/lib/use-page-title";
import { Radio } from "@base-ui/react/radio";
import { RadioGroup } from "@base-ui/react/radio-group";
import { useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { ArrowUpRight, Check } from "lucide-react";
import { useMemo } from "react";
import { Controller, useForm } from "react-hook-form";
import { z } from "zod";
import { useApi } from "@/api/context";
import { describeError, isApiError } from "@/api/errors";
import { qk, useSession } from "@/api/queries";
import type { AccentKey, Me, ProfileTheme } from "@/api/types";
import { Page, PageHeader, Section } from "@/components/layout";
import { ProfileBanner } from "@/components/profile-banner";
import { ACCENT_KEYS } from "@/features/appearance";
import { m } from "@/i18n";
import { cn } from "@/lib/cn";
import { useSaveState } from "@/lib/use-save-state";
import { Avatar } from "@/ui/avatar";
import { Button, buttonClass } from "@/ui/button";
import { Field } from "@/ui/field";
import { Input, Textarea } from "@/ui/input";
import { SaveStatus } from "@/ui/save-status";
import { toast } from "@/ui/toast";

const THEMES: ProfileTheme[] = ["cosmic", "aurora", "sunset"];
const THEME_LABEL: Record<ProfileTheme, () => string> = {
	cosmic: () => m.profile_theme_cosmic(),
	aurora: () => m.profile_theme_aurora(),
	sunset: () => m.profile_theme_sunset(),
};
const ACCENT_LABEL: Record<AccentKey, () => string> = {
	blue: () => m.accent_blue(),
	mint: () => m.accent_mint(),
	coral: () => m.accent_coral(),
	violet: () => m.accent_violet(),
	gold: () => m.accent_gold(),
	white: () => m.accent_white(),
};
const ABOUT_MAX = 160;

export function ProfileEdit() {
	usePageTitle(m.nav_profile());
	const me = useSession().data?.account;
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
			{me ? <ProfileForm me={me} /> : null}
		</Page>
	);
}

function ProfileForm({ me }: { me: Me }) {
	const api = useApi();
	const qc = useQueryClient();
	const save = useSaveState();
	const schema = useMemo(
		() =>
			z.object({
				displayName: z
					.string()
					.trim()
					.min(1, { error: m.profile_name_required() })
					.max(40, { error: m.auth_display_name_long() }),
				about: z.string().max(ABOUT_MAX, { error: m.profile_about_long({ max: ABOUT_MAX }) }),
				theme: z.enum(["cosmic", "aurora", "sunset"]),
				accent: z.enum(["blue", "mint", "coral", "violet", "gold", "white"]),
			}),
		[],
	);
	type Values = z.infer<typeof schema>;
	const {
		register,
		control,
		handleSubmit,
		watch,
		reset,
		setError,
		formState: { errors, isDirty, isSubmitting },
	} = useForm<Values>({
		resolver: zodResolver(schema),
		defaultValues: {
			displayName: me.displayName,
			about: me.about,
			theme: me.theme,
			accent: me.accent,
		},
	});
	const live = watch();

	const submit = handleSubmit(async (values) => {
		save.saving();
		try {
			const next = await api.account.updateProfile(values);
			qc.setQueryData(qk.session, (s: { account: Me | null } | undefined) =>
				s ? { ...s, account: next } : s,
			);
			void qc.invalidateQueries({ queryKey: qk.activity });
			reset(values);
			save.saved();
		} catch (e) {
			save.fail();
			if (isApiError(e) && Object.keys(e.fields).length > 0) {
				for (const [k, v] of Object.entries(e.fields)) setError(k as keyof Values, { message: v });
			} else toast.error(describeError(e));
		}
	});

	return (
		<form onSubmit={submit} noValidate className="flex flex-col gap-8">
			<Section title={m.profile_preview()} description={m.profile_preview_lead()} flush>
				<ProfileBanner theme={live.theme} className="h-24" />
				<div className="flex items-start gap-4 px-5 pb-5">
					<Avatar
						name={live.displayName || me.handle}
						accent={live.accent}
						size="lg"
						className="-mt-8 rounded-full ring-4 ring-surface"
					/>
					<div className="min-w-0 pt-2.5">
						<p className="truncate text-lead font-semibold text-fg">{live.displayName || me.handle}</p>
						<p className="truncate font-mono text-small text-fg-muted">@{me.handle}</p>
						{live.about ? <p className="mt-2 max-w-[56ch] text-body text-fg">{live.about}</p> : null}
					</div>
				</div>
			</Section>

			<Section title={m.profile_details()} description={m.profile_details_lead()}>
				<div className="flex flex-col gap-5">
					<Field label={m.auth_display_name_label()} error={errors.displayName?.message}>
						<Input autoComplete="name" maxLength={60} {...register("displayName")} />
					</Field>
					<Field
						label={m.profile_about_label()}
						description={m.profile_about_hint({ count: ABOUT_MAX - (live.about?.length ?? 0) })}
						error={errors.about?.message}
					>
						<Textarea rows={3} {...register("about")} />
					</Field>
					<Field label={m.auth_handle_label()} description={m.profile_handle_hint()}>
						<Input value={`@${me.handle}`} readOnly className="font-mono" />
					</Field>
				</div>
			</Section>

			<Section title={m.profile_look()} description={m.profile_look_lead()}>
				<div className="flex flex-col gap-6">
					<Controller
						control={control}
						name="theme"
						render={({ field }) => (
							<RadioGroup
								value={field.value}
								onValueChange={(v) => field.onChange(v)}
								aria-labelledby="theme-label"
								className="flex flex-col gap-2"
							>
								<span id="theme-label" className="text-small font-medium text-fg">
									{m.profile_theme()}
								</span>
								<div className="grid grid-cols-3 gap-3">
									{THEMES.map((t) => (
										<Radio.Root
											key={t}
											value={t}
											className="press group flex flex-col gap-2 rounded-lg p-1.5 text-start shadow-[inset_0_0_0_1px_var(--control-border)] data-[checked]:shadow-[inset_0_0_0_2px_var(--accent-ring)]"
										>
											<ProfileBanner theme={t} className="h-14 rounded-[0.5rem]" />
											<span className="flex items-center justify-between px-1 pb-0.5 text-small font-medium text-fg">
												{THEME_LABEL[t]()}
												<Radio.Indicator className="flex text-accent-text">
													<Check aria-hidden className="size-3.5" strokeWidth={2.5} />
												</Radio.Indicator>
											</span>
										</Radio.Root>
									))}
								</div>
							</RadioGroup>
						)}
					/>
					<Controller
						control={control}
						name="accent"
						render={({ field }) => (
							<RadioGroup
								value={field.value}
								onValueChange={(v) => field.onChange(v)}
								aria-labelledby="accent-label"
								className="flex flex-col gap-2"
							>
								<span id="accent-label" className="text-small font-medium text-fg">
									{m.accent_label()}
								</span>
								<div className="flex flex-wrap gap-3">
									{ACCENT_KEYS.map((k) => (
										<Radio.Root
											key={k}
											value={k}
											aria-label={ACCENT_LABEL[k]()}
											className={cn(
												"press relative flex size-9 items-center justify-center rounded-full coarse:size-11",
												"shadow-[inset_0_0_0_1px_rgb(0_0_0/0.2)] data-[checked]:shadow-[0_0_0_2px_var(--bg),0_0_0_4px_var(--fg)]",
											)}
											style={{ background: `var(--swatch-${k})` }}
										>
											<Radio.Indicator className="flex text-[#111]">
												<Check aria-hidden className="size-4" strokeWidth={3} />
											</Radio.Indicator>
										</Radio.Root>
									))}
								</div>
							</RadioGroup>
						)}
					/>
				</div>
			</Section>

			<div className="sticky bottom-0 -mx-4 flex items-center justify-between gap-4 border-t border-line bg-page/90 px-4 py-3 backdrop-blur-md sm:-mx-0 sm:rounded-xl sm:border sm:px-5">
				<SaveStatus state={save.state} />
				<div className="flex items-center gap-3">
					<Button
						type="button"
						variant="ghost"
						disabled={!isDirty || isSubmitting}
						onClick={() => reset()}
					>
						{m.profile_discard()}
					</Button>
					<Button type="submit" variant="primary" loading={isSubmitting} disabled={!isDirty}>
						{m.common_save()}
					</Button>
				</div>
			</div>
		</form>
	);
}
