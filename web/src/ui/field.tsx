import { Field as BaseField } from "@base-ui/react/field";
import { CircleAlert } from "lucide-react";
import type { ReactNode } from "react";
import { cn } from "@/lib/cn";

export interface FieldProps {
	label: ReactNode;
	description?: ReactNode;
	/** The message to show under the control; setting it marks the control invalid. */
	error?: ReactNode;
	/** Hides nothing; adds "Optional" after the label so required is the default. */
	optional?: boolean;
	name?: string;
	className?: string;
	children: ReactNode;
}

/**
 * Label, control, hint and error as one unit. The label is always visible; the placeholder is an
 * example, never the label. The error sits under the field it belongs to, with an icon, so it
 * does not rely on colour. Base UI wires the ids, `aria-describedby` and `aria-invalid`.
 */
export function Field({
	label,
	description,
	error,
	optional,
	name,
	className,
	children,
}: FieldProps) {
	return (
		<BaseField.Root
			name={name}
			invalid={Boolean(error)}
			className={cn("flex flex-col gap-1.5", className)}
		>
			<BaseField.Label className="text-small font-medium text-fg">
				{label}
				{optional ? <span className="ms-1.5 font-normal text-fg-muted">Optional</span> : null}
			</BaseField.Label>
			{children}
			{description ? (
				<BaseField.Description className="text-small text-fg-muted">
					{description}
				</BaseField.Description>
			) : null}
			{error ? (
				<BaseField.Error
					match
					data-field-error=""
					className="flex items-start gap-1.5 text-small text-danger-text"
				>
					<CircleAlert aria-hidden className="mt-px size-3.5 shrink-0" />
					<span>{error}</span>
				</BaseField.Error>
			) : null}
		</BaseField.Root>
	);
}
