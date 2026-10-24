import { Tabs as BaseTabs } from "@base-ui/react/tabs";
import type { ComponentProps } from "react";
import { cn } from "@/lib/cn";

export const Tabs = BaseTabs.Root;

/** Underline tabs. The indicator slides between tabs; the panels swap with no animation. */
export function TabsList({ className, children, ...props }: ComponentProps<typeof BaseTabs.List>) {
	return (
		<BaseTabs.List
			{...props}
			className={cn("relative flex gap-1 border-b border-line", className as string)}
		>
			{children}
			<BaseTabs.Indicator
				className={cn(
					"absolute bottom-0 start-0 h-0.5 w-[var(--active-tab-width)] translate-x-[var(--active-tab-left)] rounded-full bg-fg",
					"transition-[translate,width] duration-[var(--dur-regular)] ease-[var(--ease-out)] motion-reduce:transition-none",
				)}
			/>
		</BaseTabs.List>
	);
}

export function Tab({ className, ...props }: ComponentProps<typeof BaseTabs.Tab>) {
	return (
		<BaseTabs.Tab
			{...props}
			className={cn(
				"press flex h-10 items-center gap-2 rounded-t-md px-3 text-body font-medium text-fg-muted outline-none",
				"data-[active]:text-fg pointer-hover:hover:text-fg focus-visible:outline-2 focus-visible:outline-accent-ring",
				className as string,
			)}
		/>
	);
}

export const TabsPanel = BaseTabs.Panel;
