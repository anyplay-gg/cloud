import { cn } from "@/lib/cn";

/** The mark: an A drawn as one open stroke with a dot for the crossbar. */
export function LogoMark({ className }: { className?: string }) {
	return (
		<svg viewBox="0 0 32 32" aria-hidden className={cn("size-7 shrink-0", className)}>
			<rect width="32" height="32" rx="9" className="fill-ink" />
			<path
				d="M8.5 24 16 8l7.5 16"
				fill="none"
				strokeWidth="3"
				strokeLinecap="round"
				strokeLinejoin="round"
				className="stroke-on-ink"
			/>
			<circle cx="16" cy="20" r="2" className="fill-accent" />
		</svg>
	);
}

export function Wordmark({ className }: { className?: string }) {
	return (
		<span className={cn("inline-flex items-center gap-2.5", className)}>
			<LogoMark />
			<span className="text-prose font-semibold tracking-[-0.02em] text-fg">AnyPlay</span>
		</span>
	);
}
