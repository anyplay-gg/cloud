import { Menu as BaseMenu } from "@base-ui/react/menu";
import { Check } from "lucide-react";
import type { ComponentProps, ReactNode } from "react";
import { cn } from "@/lib/cn";

export const Menu = BaseMenu.Root;
export const MenuTrigger = BaseMenu.Trigger;
export const MenuRadioGroup = BaseMenu.RadioGroup;

const itemClass =
	"flex min-h-8 cursor-default select-none items-center gap-2.5 rounded-sm px-2.5 py-1.5 text-body outline-none " +
	"data-[highlighted]:bg-hover data-[disabled]:opacity-50 coarse:min-h-11 [&_svg]:size-4 [&_svg]:shrink-0 [&_svg]:text-fg-muted";

/** A dropdown that grows out of its trigger. Items are 8px radius inside a 14px popup with 6px padding. */
export function MenuContent({
	children,
	align = "end",
	side = "bottom",
	className,
}: {
	children: ReactNode;
	align?: "start" | "center" | "end";
	side?: "top" | "bottom" | "left" | "right";
	className?: string;
}) {
	return (
		<BaseMenu.Portal>
			<BaseMenu.Positioner sideOffset={8} align={align} side={side} className="z-50 outline-none">
				<BaseMenu.Popup
					className={cn(
						"pop min-w-52 max-w-[calc(100vw-1.5rem)] rounded-lg bg-surface p-1.5 text-fg shadow-pop outline-none",
						className,
					)}
				>
					{children}
				</BaseMenu.Popup>
			</BaseMenu.Positioner>
		</BaseMenu.Portal>
	);
}

export function MenuItem({ className, ...props }: ComponentProps<typeof BaseMenu.Item>) {
	return <BaseMenu.Item className={cn(itemClass, className as string)} {...props} />;
}

export function MenuRadioItem({
	children,
	className,
	leading,
	...props
}: ComponentProps<typeof BaseMenu.RadioItem> & { leading?: ReactNode }) {
	return (
		<BaseMenu.RadioItem className={cn(itemClass, className as string)} {...props}>
			{leading}
			<span className="flex-1">{children}</span>
			<BaseMenu.RadioItemIndicator className="flex">
				<Check aria-hidden className="!text-accent-text" strokeWidth={2.25} />
			</BaseMenu.RadioItemIndicator>
		</BaseMenu.RadioItem>
	);
}

export function MenuLabel({ children }: { children: ReactNode }) {
	return (
		<BaseMenu.Group>
			<BaseMenu.GroupLabel className="px-2.5 pb-1 pt-2 text-caption font-medium text-fg-muted">
				{children}
			</BaseMenu.GroupLabel>
		</BaseMenu.Group>
	);
}

export const MenuGroup = BaseMenu.Group;
export const MenuGroupLabel = ({ children }: { children: ReactNode }) => (
	<BaseMenu.GroupLabel className="px-2.5 pb-1 pt-2 text-caption font-medium text-fg-muted">
		{children}
	</BaseMenu.GroupLabel>
);

export function MenuSeparator() {
	return <BaseMenu.Separator className="-mx-1.5 my-1.5 h-px bg-line" />;
}
