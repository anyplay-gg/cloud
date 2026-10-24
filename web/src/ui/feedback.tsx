import { Progress as BaseProgress } from "@base-ui/react/progress";
import { CircleAlert, RefreshCw } from "lucide-react";
import type { ReactNode } from "react";
import { cn } from "@/lib/cn";
import { Button } from "./button";

export function Skeleton({ className }: { className?: string }) {
	return <div aria-hidden className={cn("skeleton rounded-md bg-hover", className)} />;
}

/** Announces itself once so a screen reader knows the section is loading, not empty. */
export function LoadingRegion({ label, children }: { label: string; children: ReactNode }) {
	return (
		<div role="status" aria-live="polite" aria-busy="true">
			<span className="sr-only">{label}</span>
			{children}
		</div>
	);
}

export interface EmptyStateProps {
	icon?: ReactNode;
	title: string;
	description?: ReactNode;
	action?: ReactNode;
	className?: string;
}

/** Says what this place is, how to fill it, and offers one next step. */
export function EmptyState({ icon, title, description, action, className }: EmptyStateProps) {
	return (
		<div
			className={cn(
				"flex flex-col items-start gap-4 px-5 py-8 sm:flex-row sm:items-center sm:gap-5 sm:px-6",
				className,
			)}
		>
			{icon ? (
				<div className="flex size-11 shrink-0 items-center justify-center rounded-lg bg-raised text-fg-muted shadow-[inset_0_0_0_1px_var(--line-strong)] [&_svg]:size-5">
					{icon}
				</div>
			) : null}
			<div className="min-w-0 flex-1">
				<p className="text-body font-semibold text-fg">{title}</p>
				{description ? (
					<p className="mt-1 max-w-[56ch] text-body text-fg-muted">{description}</p>
				) : null}
			</div>
			{action ? <div className="shrink-0">{action}</div> : null}
		</div>
	);
}

/** A failed load: says what failed and offers the retry. */
export function ErrorState({
	title = "Couldn't load this",
	description = "Check your connection and try again.",
	onRetry,
	retrying,
	className,
}: {
	title?: string;
	description?: ReactNode;
	onRetry?: () => void;
	retrying?: boolean;
	className?: string;
}) {
	return (
		<div
			role="alert"
			className={cn(
				"flex flex-col items-start gap-4 px-5 py-8 sm:flex-row sm:items-center sm:gap-5 sm:px-6",
				className,
			)}
		>
			<div className="flex size-11 shrink-0 items-center justify-center rounded-lg bg-danger-soft text-danger-text shadow-[inset_0_0_0_1px_var(--danger-7)]">
				<CircleAlert aria-hidden className="size-5" />
			</div>
			<div className="min-w-0 flex-1">
				<p className="text-body font-semibold text-fg">{title}</p>
				<p className="mt-1 max-w-[56ch] text-body text-fg-muted">{description}</p>
			</div>
			{onRetry ? (
				<Button variant="secondary" onClick={onRetry} loading={retrying}>
					{retrying ? null : <RefreshCw aria-hidden />}
					Try again
				</Button>
			) : null}
		</div>
	);
}

export interface ProgressBarProps {
	value: number;
	label: string;
	className?: string;
	/** Draw the glow the launcher puts on its progress fill. */
	glow?: boolean;
	size?: "sm" | "md";
}

/**
 * Accent fill on a quiet track. The fill scales on the compositor (transform), never its width.
 */
export function ProgressBar({
	value,
	label,
	className,
	glow = true,
	size = "md",
}: ProgressBarProps) {
	const v = Math.max(0, Math.min(100, value));
	return (
		<BaseProgress.Root value={v} aria-label={label} className={className}>
			<BaseProgress.Track
				className={cn(
					"relative block w-full overflow-hidden rounded-full bg-track",
					size === "md" ? "h-1.5" : "h-1",
				)}
			>
				<span
					className={cn(
						"absolute inset-0 origin-left rounded-full bg-accent transition-transform duration-[var(--dur-slow)] ease-[var(--ease-out)] rtl:origin-right motion-reduce:transition-none",
						glow && "shadow-[0_0_6px_color-mix(in_oklab,var(--accent)_60%,transparent)]",
					)}
					style={{ transform: `scaleX(${v / 100})` }}
				/>
			</BaseProgress.Track>
		</BaseProgress.Root>
	);
}
