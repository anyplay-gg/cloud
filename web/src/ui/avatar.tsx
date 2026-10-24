import { Avatar as BaseAvatar } from "@base-ui/react/avatar";
import type { AccentKey } from "@/api/types";
import { cn } from "@/lib/cn";

const SIZE = {
	xs: "size-6 text-[0.625rem]",
	sm: "size-8 text-caption",
	md: "size-10 text-body",
	lg: "size-14 text-title",
	xl: "size-24 text-display",
} as const;

export function initialsOf(name: string): string {
	const parts = name.trim().split(/\s+/).filter(Boolean);
	if (parts.length === 0) return "?";
	const first = parts[0]?.[0] ?? "?";
	const last = parts.length > 1 ? (parts[parts.length - 1]?.[0] ?? "") : "";
	return (first + last).toUpperCase();
}

export interface AvatarProps {
	name: string;
	accent: AccentKey;
	src?: string;
	size?: keyof typeof SIZE;
	className?: string;
	/** Adds a status dot: the one other place colour means something besides progress. */
	status?: "online" | "away" | "offline" | null;
}

/**
 * Initials on the player's own launcher accent (the launcher's avatar gradient). Uploads are not
 * part of the first version; `src` is here so they slot in without a new component.
 */
export function Avatar({ name, accent, src, size = "md", className, status }: AvatarProps) {
	return (
		<span className={cn("relative inline-flex shrink-0", className)} data-person-accent={accent}>
			<BaseAvatar.Root
				className={cn(
					"inline-flex select-none items-center justify-center overflow-hidden rounded-full font-semibold text-[#111] ring-1 ring-inset ring-[rgb(0_0_0/0.12)]",
					"bg-[linear-gradient(145deg,color-mix(in_oklab,var(--pa),white_30%),var(--pa))]",
					SIZE[size],
				)}
			>
				{src ? <BaseAvatar.Image src={src} alt="" className="size-full object-cover" /> : null}
				<BaseAvatar.Fallback aria-hidden>{initialsOf(name)}</BaseAvatar.Fallback>
			</BaseAvatar.Root>
			{status ? <StatusDot status={status} size={size} /> : null}
		</span>
	);
}

export function StatusDot({
	status,
	size = "md",
}: {
	status: "online" | "away" | "offline";
	size?: keyof typeof SIZE;
}) {
	const label = status === "online" ? "Online" : status === "away" ? "Away" : "Offline";
	const big = size === "lg" || size === "xl";
	return (
		<span
			role="img"
			aria-label={label}
			className={cn(
				"absolute bottom-0 end-0 rounded-full ring-2 ring-surface",
				big ? "size-3.5" : "size-2.5",
				status === "online" && "bg-accent shadow-[0_0_6px_var(--accent)]",
				status === "away" && "bg-warning",
				status === "offline" && "bg-fg-faint",
			)}
		/>
	);
}

/** The same dot, inline: for a line of text rather than the corner of an avatar. */
export function PresenceDot({ status }: { status: "online" | "away" | "offline" }) {
	return (
		<span
			aria-hidden
			className={cn(
				"inline-block size-2.5 shrink-0 rounded-full",
				status === "online" && "bg-accent shadow-[0_0_6px_var(--accent)]",
				status === "away" && "bg-warning",
				status === "offline" && "bg-fg-faint",
			)}
		/>
	);
}
