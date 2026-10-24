import { Checkbox as BaseCheckbox } from "@base-ui/react/checkbox";
import { Switch as BaseSwitch } from "@base-ui/react/switch";
import { Check } from "lucide-react";
import type { ComponentProps } from "react";
import { cn } from "@/lib/cn";

/**
 * The launcher's switch: a capsule whose track takes the accent when on, with a knob that
 * slides. The track is 44 wide so the target is comfortable; the row it sits in is the real
 * hit area (wrap it in a label).
 */
export function Switch({ className, ...props }: ComponentProps<typeof BaseSwitch.Root>) {
	return (
		<BaseSwitch.Root
			{...props}
			className={cn(
				"press relative inline-flex h-6 w-11 shrink-0 items-center rounded-full p-0.5",
				"bg-active shadow-[inset_0_0_0_1px_var(--control-border)] transition-[background-color,box-shadow] duration-[var(--dur-regular)] ease-[var(--ease-out)]",
				"data-[checked]:bg-accent data-[checked]:shadow-none data-[disabled]:opacity-50",
				className as string,
			)}
		>
			<BaseSwitch.Thumb
				className={cn(
					"size-5 rounded-full bg-fg-muted shadow-knob transition-[transform,background-color] duration-[var(--dur-regular)] ease-[var(--ease-out)]",
					"data-[checked]:translate-x-5 data-[checked]:bg-on-accent rtl:data-[checked]:-translate-x-5",
				)}
			/>
		</BaseSwitch.Root>
	);
}

export function Checkbox({ className, ...props }: ComponentProps<typeof BaseCheckbox.Root>) {
	return (
		<BaseCheckbox.Root
			{...props}
			className={cn(
				"press flex size-[1.125rem] shrink-0 items-center justify-center rounded-[5px] bg-raised",
				"shadow-[inset_0_0_0_1px_var(--control-border)] transition-[background-color,box-shadow] duration-[var(--dur-quick)] ease-[var(--ease-out)]",
				"data-[checked]:bg-accent data-[checked]:shadow-none data-[disabled]:opacity-50",
				className as string,
			)}
		>
			<BaseCheckbox.Indicator className="flex text-on-accent">
				<Check aria-hidden className="size-3.5" strokeWidth={3} />
			</BaseCheckbox.Indicator>
		</BaseCheckbox.Root>
	);
}
