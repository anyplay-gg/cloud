import { Select as BaseSelect } from "@base-ui/react/select";
import { Check, ChevronsUpDown } from "lucide-react";
import { cn } from "@/lib/cn";
import { controlClass } from "./input";

export interface SelectOption<T extends string> {
	value: T;
	label: string;
	description?: string;
}

export interface SelectProps<T extends string> {
	value: T;
	onValueChange: (value: T) => void;
	options: readonly SelectOption<T>[];
	name?: string;
	disabled?: boolean;
	className?: string;
	"aria-label"?: string;
	id?: string;
}

/** A listbox that opens out of its trigger. Options may carry a one-line description. */
export function Select<T extends string>({
	value,
	onValueChange,
	options,
	name,
	disabled,
	className,
	id,
	"aria-label": ariaLabel,
}: SelectProps<T>) {
	return (
		<BaseSelect.Root
			value={value}
			onValueChange={(v) => {
				if (v !== null) onValueChange(v as T);
			}}
			items={options.map((o) => ({ value: o.value, label: o.label }))}
			name={name}
			disabled={disabled}
		>
			<BaseSelect.Trigger
				id={id}
				aria-label={ariaLabel}
				className={cn(
					controlClass,
					"press flex h-9 items-center justify-between gap-2 px-3 text-start text-body coarse:h-11",
					"data-[popup-open]:bg-hover",
					className,
				)}
			>
				<BaseSelect.Value className="truncate" />
				<BaseSelect.Icon className="text-fg-muted">
					<ChevronsUpDown aria-hidden className="size-4" />
				</BaseSelect.Icon>
			</BaseSelect.Trigger>
			<BaseSelect.Portal>
				<BaseSelect.Positioner
					sideOffset={6}
					alignItemWithTrigger={false}
					className="z-50 outline-none"
				>
					<BaseSelect.Popup
						className={cn(
							"pop min-w-[var(--anchor-width)] max-w-[min(22rem,calc(100vw-2rem))] origin-[var(--transform-origin)] rounded-lg bg-surface p-1.5 shadow-pop outline-none",
						)}
					>
						<BaseSelect.List className="flex max-h-72 flex-col gap-0.5 overflow-auto">
							{options.map((o) => (
								<BaseSelect.Item
									key={o.value}
									value={o.value}
									className={cn(
										"flex cursor-default select-none items-start gap-2 rounded-sm py-2 ps-2 pe-2.5 text-body outline-none coarse:py-3",
										"data-[highlighted]:bg-hover",
									)}
								>
									<span className="mt-0.5 size-4 shrink-0 text-accent-text">
										<BaseSelect.ItemIndicator>
											<Check aria-hidden className="size-4" strokeWidth={2.25} />
										</BaseSelect.ItemIndicator>
									</span>
									<span className="flex min-w-0 flex-1 flex-col">
										<BaseSelect.ItemText>{o.label}</BaseSelect.ItemText>
										{o.description ? (
											<span className="text-small text-fg-muted">{o.description}</span>
										) : null}
									</span>
								</BaseSelect.Item>
							))}
						</BaseSelect.List>
					</BaseSelect.Popup>
				</BaseSelect.Positioner>
			</BaseSelect.Portal>
		</BaseSelect.Root>
	);
}
