import type { ReactNode } from "react";
import { cn } from "@/lib/cn";

/** The content column of a screen. Settings read at 44rem; list screens use the wide column. */
export function Page({
	children,
	width = "narrow",
	className,
}: {
	children: ReactNode;
	width?: "narrow" | "wide";
	className?: string;
}) {
	return (
		<div
			className={cn(
				"mx-auto flex w-full flex-col gap-10",
				width === "narrow" ? "max-w-[44rem]" : "max-w-[60rem]",
				className,
			)}
		>
			{children}
		</div>
	);
}

export function PageHeader({
	title,
	description,
	actions,
}: {
	title: ReactNode;
	description?: ReactNode;
	actions?: ReactNode;
}) {
	return (
		<header className="flex flex-col gap-4 sm:flex-row sm:items-end sm:justify-between">
			<div className="min-w-0">
				<h1 className="text-heading font-semibold text-fg">{title}</h1>
				{description ? (
					<p className="mt-1.5 max-w-[60ch] text-body text-fg-muted">{description}</p>
				) : null}
			</div>
			{actions ? <div className="flex shrink-0 items-center gap-3">{actions}</div> : null}
		</header>
	);
}

export interface SectionProps {
	title: ReactNode;
	description?: ReactNode;
	/** Sits at the end of the header: a primary action for the whole section. */
	actions?: ReactNode;
	children?: ReactNode;
	tone?: "default" | "danger";
	/** Rows manage their own padding and dividers. */
	flush?: boolean;
	id?: string;
	className?: string;
}

/**
 * A titled card. The title and description sit above the surface in the page's own rhythm, so a
 * screen reads as a list of sections rather than a stack of boxes within boxes.
 */
export function Section({
	title,
	description,
	actions,
	children,
	tone = "default",
	flush,
	id,
	className,
}: SectionProps) {
	const headingId = id ? `${id}-title` : undefined;
	return (
		<section aria-labelledby={headingId} id={id} className={cn("flex flex-col gap-3", className)}>
			<div className="flex items-center justify-between gap-4">
				<div className="min-w-0">
					<h2
						id={headingId}
						className={cn(
							"text-prose font-semibold",
							tone === "danger" ? "text-danger-text" : "text-fg",
						)}
					>
						{title}
					</h2>
					{description ? (
						<p className="mt-1 max-w-[60ch] text-body text-fg-muted">{description}</p>
					) : null}
				</div>
				{actions ? <div className="shrink-0">{actions}</div> : null}
			</div>
			{children ? (
				<div
					className={cn(
						"overflow-hidden rounded-xl bg-surface shadow-card",
						tone === "danger" && "shadow-[0_0_0_1px_var(--danger-7)]",
						flush ? "" : "p-5",
					)}
				>
					{children}
				</div>
			) : null}
		</section>
	);
}

/** One line of a settings list: what it is on the left, the control on the right. */
export function Row({
	title,
	description,
	control,
	children,
	className,
	as: Tag = "div",
}: {
	title: ReactNode;
	description?: ReactNode;
	control?: ReactNode;
	children?: ReactNode;
	className?: string;
	as?: "div" | "li";
}) {
	return (
		<Tag
			className={cn(
				"flex flex-col gap-3 px-5 py-4 sm:flex-row sm:items-center sm:justify-between sm:gap-6",
				className,
			)}
		>
			<div className="min-w-0 flex-1">
				<div className="text-body font-medium text-fg">{title}</div>
				{description ? <div className="mt-0.5 text-body text-fg-muted">{description}</div> : null}
				{children}
			</div>
			{control ? <div className="flex shrink-0 items-center gap-3">{control}</div> : null}
		</Tag>
	);
}

/** Rows separated by hairlines. */
export function RowList({
	children,
	className,
	label,
}: {
	children: ReactNode;
	className?: string;
	label?: string;
}) {
	return (
		<ul aria-label={label} className={cn("divide-y divide-line", className)}>
			{children}
		</ul>
	);
}
