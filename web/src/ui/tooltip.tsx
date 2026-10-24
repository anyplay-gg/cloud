import { Tooltip as BaseTooltip } from "@base-ui/react/tooltip";
import type { ReactElement, ReactNode } from "react";

/** One provider for the app: the first tooltip waits, the ones next to it open at once. */
export function TooltipProvider({ children }: { children: ReactNode }) {
	return (
		<BaseTooltip.Provider delay={450} closeDelay={80}>
			{children}
		</BaseTooltip.Provider>
	);
}

/**
 * Names an icon-only control or spells out a truncated value. Never the only place a fact lives:
 * touch devices have no hover.
 */
export function Tooltip({
	label,
	children,
	side = "top",
}: {
	label: ReactNode;
	children: ReactElement;
	side?: "top" | "bottom" | "left" | "right";
}) {
	return (
		<BaseTooltip.Root>
			<BaseTooltip.Trigger render={children} />
			<BaseTooltip.Portal>
				<BaseTooltip.Positioner side={side} sideOffset={8} className="z-[60]">
					<BaseTooltip.Popup className="pop data-[instant]:duration-0 max-w-64 rounded-md bg-ink px-2.5 py-1.5 text-small font-medium text-on-ink shadow-pop">
						{label}
					</BaseTooltip.Popup>
				</BaseTooltip.Positioner>
			</BaseTooltip.Portal>
		</BaseTooltip.Root>
	);
}
