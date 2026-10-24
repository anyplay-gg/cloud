import { Field as BaseField } from "@base-ui/react/field";
import { Input as BaseInput } from "@base-ui/react/input";
import type { ComponentProps, Ref } from "react";
import { cn } from "@/lib/cn";

export const controlClass =
	"w-full rounded-md bg-raised text-fg placeholder:text-fg-muted " +
	"shadow-[inset_0_0_0_1px_var(--control-border)] transition-[box-shadow,background-color] duration-[var(--dur-quick)] ease-[var(--ease-out)] " +
	"pointer-hover:hover:bg-hover disabled:opacity-50 aria-[invalid=true]:shadow-[inset_0_0_0_1px_var(--danger-9)] " +
	"data-[invalid]:shadow-[inset_0_0_0_1px_var(--danger-9)] read-only:bg-surface";

export function Input({
	className,
	ref,
	...props
}: ComponentProps<typeof BaseInput> & { ref?: Ref<HTMLInputElement> }) {
	return (
		<BaseInput
			ref={ref}
			className={cn(controlClass, "h-9 px-3 text-body coarse:h-11", className as string)}
			{...props}
		/>
	);
}

/** A multi-line control. Use it inside <Field>, which wires the label and the error. */
export function Textarea({
	className,
	ref,
	...props
}: ComponentProps<"textarea"> & { ref?: Ref<HTMLTextAreaElement> }) {
	return (
		<BaseField.Control
			render={
				<textarea
					ref={ref}
					className={cn(controlClass, "min-h-24 resize-y px-3 py-2 text-body", className)}
					{...props}
				/>
			}
		/>
	);
}
