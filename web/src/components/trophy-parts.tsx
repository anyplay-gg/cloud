import type { GradeCounts } from "@/api/types";
import { m } from "@/i18n";
import { cn } from "@/lib/cn";
import { formatNumber } from "@/lib/format";
import { GRADE_LABEL, GRADES, type Grade } from "@/lib/trophies";
import { RollingNumber } from "@/ui/rolling-number";

const FILL: Record<Grade, string> = {
	platinum: "bg-platinum",
	gold: "bg-gold",
	silver: "bg-silver",
	bronze: "bg-bronze",
};

/** A grade is a colour and a word, never colour alone. No trophy art ever reaches the cloud. */
export function GradeDot({ grade, className }: { grade: Grade; className?: string }) {
	return (
		<span
			aria-hidden
			className={cn(
				"inline-block size-2.5 shrink-0 rounded-full shadow-[inset_0_0_0_1px_rgb(0_0_0/0.14)]",
				FILL[grade],
				className,
			)}
		/>
	);
}

/** "3 platinum, 12 gold, ..." as four compact counts. */
export function GradeLine({ counts, className }: { counts: GradeCounts; className?: string }) {
	return (
		<ul
			aria-label={GRADES.map((g) =>
				m.trophies_grade_count({ count: counts[g], grade: GRADE_LABEL[g].toLowerCase() }),
			).join(", ")}
			className={cn(
				"flex flex-wrap items-center gap-x-4 gap-y-1 text-small text-fg-muted",
				className,
			)}
		>
			{GRADES.map((g) => (
				<li key={g} aria-hidden className="flex items-center gap-1.5">
					<GradeDot grade={g} />
					<span className="nums font-medium text-fg">{formatNumber(counts[g])}</span>
					<span className="sr-only">{GRADE_LABEL[g]}</span>
				</li>
			))}
		</ul>
	);
}

/** Four tiles: how many of each grade are earned. */
export function GradeTiles({ counts }: { counts: GradeCounts }) {
	return (
		<ul className="grid grid-cols-2 gap-px overflow-hidden rounded-lg bg-line sm:grid-cols-4">
			{GRADES.map((g) => (
				<li key={g} className="flex flex-col gap-1 bg-surface p-4">
					<span className="flex items-center gap-2 text-small font-medium text-fg-muted">
						<GradeDot grade={g} />
						{GRADE_LABEL[g]}
					</span>
					<span className="text-heading font-semibold text-fg">
						<RollingNumber value={counts[g]} />
					</span>
				</li>
			))}
		</ul>
	);
}
