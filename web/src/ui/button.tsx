import { Button as BaseButton } from "@base-ui/react/button";
import { Loader2 } from "lucide-react";
import type { ComponentProps } from "react";
import { cn } from "@/lib/cn";

export type ButtonVariant = "primary" | "secondary" | "ghost" | "danger" | "danger-soft";
export type ButtonSize = "sm" | "md" | "lg";

const base =
	"press relative inline-flex shrink-0 items-center justify-center gap-2 whitespace-nowrap rounded-md font-medium select-none " +
	"disabled:pointer-events-none disabled:opacity-50 data-[disabled]:pointer-events-none data-[disabled]:opacity-50 " +
	"[&_svg]:size-4 [&_svg]:shrink-0";

const variants: Record<ButtonVariant, string> = {
	// The launcher's focused surface: solid near-white, near-black text. Ink, not accent.
	primary: "bg-ink text-on-ink pointer-hover:hover:bg-ink-hover",
	secondary:
		"bg-raised text-fg shadow-[inset_0_0_0_1px_var(--line-strong)] pointer-hover:hover:bg-hover",
	ghost: "text-fg-muted pointer-hover:hover:bg-hover pointer-hover:hover:text-fg",
	danger: "bg-danger text-on-danger pointer-hover:hover:bg-danger-hover",
	"danger-soft":
		"bg-danger-soft text-danger-text shadow-[inset_0_0_0_1px_var(--danger-7)] pointer-hover:hover:bg-[var(--danger-4)]",
};

const sizes: Record<ButtonSize, string> = {
	sm: "h-8 px-3 text-small coarse:h-11",
	md: "h-9 px-3.5 text-body coarse:h-11",
	lg: "h-11 px-5 text-body",
};

const iconSizes: Record<ButtonSize, string> = {
	sm: "size-8 coarse:size-11",
	md: "size-9 coarse:size-11",
	lg: "size-11",
};

export function buttonClass({
	variant = "secondary",
	size = "md",
	icon = false,
	className,
}: {
	variant?: ButtonVariant;
	size?: ButtonSize;
	icon?: boolean;
	className?: string;
} = {}) {
	return cn(
		base,
		variants[variant],
		icon ? iconSizes[size] : sizes[size],
		icon && "px-0",
		className,
	);
}

export interface ButtonProps extends Omit<ComponentProps<typeof BaseButton>, "className"> {
	variant?: ButtonVariant;
	size?: ButtonSize;
	/** Square, for a lone icon. Needs an aria-label. */
	icon?: boolean;
	/** Keeps the label, shows a spinner and ignores presses. Focus stays where it was. */
	loading?: boolean;
	className?: string;
}

export function Button({
	variant = "secondary",
	size = "md",
	icon,
	loading,
	className,
	children,
	disabled,
	onClick,
	...props
}: ButtonProps) {
	return (
		<BaseButton
			{...props}
			disabled={disabled || loading}
			focusableWhenDisabled={loading}
			aria-busy={loading || undefined}
			onClick={loading ? undefined : onClick}
			className={buttonClass({ variant, size, icon, className })}
		>
			{loading ? <Loader2 aria-hidden className="animate-spin" /> : null}
			{children}
		</BaseButton>
	);
}
