import { PreviewCard } from "@base-ui/react/preview-card";
import { Link } from "@tanstack/react-router";
import type { ReactNode } from "react";
import type { Friend } from "@/api/types";
import { m } from "@/i18n";
import { formatMonthYear } from "@/lib/format";
import { accentFor, themeFor } from "@/lib/identity";
import { Avatar, PresenceDot } from "@/ui/avatar";
import { ProfileBanner } from "./profile-banner";

/**
 * A small profile that opens from a friend's name on hover or keyboard focus (the profile hover
 * card pattern from Bencho, written again on Base UI's preview card). It shows only what the
 * list already has, so opening it costs no request. The name stays a real link either way.
 */
export function PersonHoverCard({ friend, children }: { friend: Friend; children: ReactNode }) {
	const handle = friend.account.handle;
	return (
		<PreviewCard.Root>
			<PreviewCard.Trigger
				delay={350}
				closeDelay={120}
				render={<Link to="/u/$name" params={{ name: handle }} />}
				className="truncate rounded-sm font-medium text-fg outline-offset-2 hover:underline hover:decoration-line-strong hover:underline-offset-4"
			>
				{children}
			</PreviewCard.Trigger>
			<PreviewCard.Portal>
				<PreviewCard.Positioner side="bottom" align="start" sideOffset={10} className="z-50">
					<PreviewCard.Popup className="pop w-72 overflow-hidden rounded-xl bg-surface shadow-pop outline-none">
						<ProfileBanner theme={themeFor(handle)} className="h-14" />
						<div className="px-4 pb-4">
							<Avatar
								name={handle}
								accent={accentFor(handle)}
								size="lg"
								className="-mt-7 rounded-full ring-4 ring-surface"
							/>
							<p className="mt-2 truncate font-mono text-body font-semibold text-fg">@{handle}</p>
							<p className="mt-3 flex items-center gap-2 text-small text-fg-muted">
								<PresenceDot status={friend.presence.online ? "online" : "offline"} />
								{friend.presence.activity
									? m.friends_playing({ title: friend.presence.activity.title })
									: friend.presence.online
										? m.status_online()
										: m.status_offline()}
							</p>
							<p className="mt-1 text-small text-fg-muted">
								{m.friends_since({ date: formatMonthYear(friend.since) })}
							</p>
						</div>
					</PreviewCard.Popup>
				</PreviewCard.Positioner>
			</PreviewCard.Portal>
		</PreviewCard.Root>
	);
}
