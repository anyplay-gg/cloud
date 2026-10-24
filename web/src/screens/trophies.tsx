import { Trophy } from "lucide-react";
import { usePageTitle } from "@/lib/use-page-title";
import { useTrophies } from "@/api/queries";
import type { TitleProgress, TrophySummary } from "@/api/types";
import { Page, PageHeader, Section } from "@/components/layout";
import { QueryView } from "@/components/query-state";
import { GradeLine, GradeTiles } from "@/components/trophy-parts";
import { m } from "@/i18n";
import { relativeTime } from "@/lib/format";
import { EmptyState, ProgressBar, Skeleton } from "@/ui/feedback";

export function Trophies() {
	usePageTitle(m.nav_trophies());
	const trophies = useTrophies();
	return (
		<Page width="wide">
			<PageHeader title={m.trophies_title()} description={m.trophies_lead()} />
			<QueryView
				query={trophies}
				loadingLabel={m.trophies_loading()}
				errorTitle={m.trophies_error()}
				loading={
					<div className="flex flex-col gap-8">
						<div className="rounded-xl bg-surface p-5 shadow-card">
							<Skeleton className="h-10 w-24" />
							<Skeleton className="mt-4 h-1.5 w-full" />
							<div className="mt-6 grid grid-cols-2 gap-3 sm:grid-cols-4">
								{[0, 1, 2, 3].map((i) => (
									<Skeleton key={i} className="h-16" />
								))}
							</div>
						</div>
						<div className="rounded-xl bg-surface shadow-card">
							{[0, 1, 2].map((i) => (
								<div key={i} className="space-y-3 border-t border-line p-5 first:border-t-0">
									<Skeleton className="h-4 w-40" />
									<Skeleton className="h-1.5 w-full" />
								</div>
							))}
						</div>
					</div>
				}
				isEmpty={(d) => d.titles.length === 0}
				empty={
					<div className="overflow-hidden rounded-xl bg-surface shadow-card">
						<EmptyState
							icon={<Trophy aria-hidden strokeWidth={1.75} />}
							title={m.trophies_empty_title()}
							description={m.trophies_empty_text()}
						/>
					</div>
				}
			>
				{(d) => <TrophyContent summary={d} />}
			</QueryView>
		</Page>
	);
}

function TrophyContent({ summary }: { summary: TrophySummary }) {
	const sorted = [...summary.titles].sort(
		(a, b) => (b.lastEarnedAt ?? "").localeCompare(a.lastEarnedAt ?? "") || a.name.localeCompare(b.name),
	);
	return (
		<>
			<Section title={m.trophies_overall()} description={m.trophies_overall_lead()}>
				<div className="flex flex-col gap-6">
					<div className="flex flex-col gap-3">
						<div className="flex items-baseline justify-between gap-4">
							<p className="nums text-hero font-semibold text-fg">{summary.percent}%</p>
							<p className="text-small text-fg-muted">
								{m.trophies_titles_count({ count: summary.titles.length })}
							</p>
						</div>
						<ProgressBar value={summary.percent} label={m.trophies_overall_progress()} />
					</div>
					<GradeTiles earned={summary.earned} total={summary.total} />
				</div>
			</Section>
			<Section title={m.trophies_by_title()} description={m.trophies_by_title_lead()} flush>
				<ul>
					{sorted.map((t) => (
						<TitleRow key={t.titleId} title={t} />
					))}
				</ul>
			</Section>
		</>
	);
}

function TitleRow({ title }: { title: TitleProgress }) {
	return (
		<li className="flex flex-col gap-3 border-t border-line px-5 py-4 first:border-t-0">
			<div className="flex items-start justify-between gap-4">
				<div className="min-w-0">
					<p className="truncate text-body font-medium text-fg">{title.name}</p>
					<p className="mt-0.5 font-mono text-caption text-fg-muted">{title.titleId}</p>
				</div>
				<p className="nums shrink-0 text-lead font-semibold text-fg">
					{title.percent}
					<span className="text-body font-normal text-fg-muted">%</span>
				</p>
			</div>
			<ProgressBar value={title.percent} label={m.trophies_progress_named({ name: title.name })} size="sm" glow={false} />
			<div className="flex flex-col gap-1.5 sm:flex-row sm:items-center sm:justify-between">
				<GradeLine counts={title.earned} />
				<p className="text-small text-fg-muted">
					{title.lastEarnedAt
						? m.trophies_last_earned({ when: relativeTime(title.lastEarnedAt) })
						: m.trophies_not_started()}
				</p>
			</div>
		</li>
	);
}
