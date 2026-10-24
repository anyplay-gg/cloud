import type { ProfileTheme } from "@/api/types";
import { cn } from "@/lib/cn";

/**
 * The launcher's theme gradient (middle to deep, with a glow of the bright colour) as a quiet
 * banner. No stickers, no frames: the theme is the only decoration a profile has.
 */
export function ProfileBanner({ theme, className }: { theme: ProfileTheme; className?: string }) {
	return (
		<div
			aria-hidden
			data-profile-theme={theme}
			className={cn(
				"relative overflow-hidden bg-[linear-gradient(180deg,var(--t-mid),var(--t-deep))]",
				"after:absolute after:inset-0 after:bg-[radial-gradient(55%_130%_at_82%_-10%,color-mix(in_oklab,var(--t-bright)_42%,transparent),transparent_72%)] after:content-['']",
				className,
			)}
		/>
	);
}
