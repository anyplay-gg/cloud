import type { ReactNode } from "react";
import { cn } from "@/lib/cn";

export type BadgeTone =
	| "neutral"
	| "accent"
	| "success"
	| "warning"
	| "danger"
	| "platinum"
	| "gold"
	| "silver"
	| "bronze";

const tones: Record<BadgeTone, string> = {
	neutral: "bg-raised text-fg-muted shadow-[inset_0_0_0_1px_var(--line-strong)]",
	accent: "bg-accent-soft text-accent-text shadow-[inset_0_0_0_1px_var(--accent-7)]",
	success: "bg-success-soft text-success-text shadow-[inset_0_0_0_1px_var(--success-7)]",
	warning: "bg-warning-soft text-warning-text shadow-[inset_0_0_0_1px_var(--warning-7)]",
	danger: "bg-danger-soft text-danger-text shadow-[inset_0_0_0_1px_var(--danger-7)]",
	platinum: "bg-platinum-soft text-platinum-text shadow-[inset_0_0_0_1px_var(--grade-platinum-7)]",
	gold: "bg-gold-soft text-gold-text shadow-[inset_0_0_0_1px_var(--grade-gold-7)]",
	silver: "bg-silver-soft text-silver-text shadow-[inset_0_0_0_1px_var(--grade-silver-7)]",
	bronze: "bg-bronze-soft text-bronze-text shadow-[inset_0_0_0_1px_var(--grade-bronze-7)]",
};

/** A small fact about a thing. The text says it; the colour only agrees. */
export function Badge({
	tone = "neutral",
	children,
	className,
	icon,
}: {
	tone?: BadgeTone;
	children: ReactNode;
	className?: string;
	icon?: ReactNode;
}) {
	return (
		<span
			className={cn(
				"inline-flex h-5 shrink-0 items-center gap-1 whitespace-nowrap rounded-full px-2 text-caption font-medium [&_svg]:size-3",
				tones[tone],
				className,
			)}
		>
			{icon}
			{children}
		</span>
	);
}
