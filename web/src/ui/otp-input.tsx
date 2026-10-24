import { OTPField } from "@base-ui/react/otp-field";
import { cn } from "@/lib/cn";

export interface OtpInputProps {
	value: string;
	onChange: (value: string) => void;
	onComplete?: (value: string) => void;
	length?: number;
	invalid?: boolean;
	disabled?: boolean;
	autoFocus?: boolean;
	label?: string;
	name?: string;
}

/**
 * Six boxes, one code. Typing, pasting the whole code and the e-mail app's autofill all work,
 * because the first box carries autocomplete="one-time-code". Digits are tabular and mono.
 */
export function OtpInput({
	value,
	onChange,
	onComplete,
	length = 6,
	invalid,
	disabled,
	autoFocus,
	label = "Code",
	name = "code",
}: OtpInputProps) {
	const half = Math.floor(length / 2);
	const slots = Array.from({ length }, (_, i) => `slot-${i + 1}`);
	return (
		<OTPField.Root
			length={length}
			name={name}
			value={value}
			onValueChange={(v) => onChange(v)}
			onValueComplete={(v) => onComplete?.(v)}
			disabled={disabled}
			aria-label={label}
			className="flex items-center gap-2"
		>
			{slots.map((slot, i) => (
				<span key={slot} className="contents">
					{i === half ? (
						<OTPField.Separator aria-hidden className="mx-0.5 h-px w-2 shrink-0 bg-line-strong" />
					) : null}
					<OTPField.Input
						aria-label={`${label}, digit ${i + 1} of ${length}`}
						autoFocus={autoFocus && i === 0}
						aria-invalid={invalid || undefined}
						className={cn(
							"h-12 w-10 min-w-0 flex-1 rounded-md bg-raised text-center font-mono text-lead font-medium text-fg nums sm:h-14 sm:w-12 sm:flex-none",
							"shadow-[inset_0_0_0_1px_var(--control-border)] transition-[box-shadow,background-color] duration-[var(--dur-quick)] ease-[var(--ease-out)]",
							"halo caret-accent focus:bg-hover aria-[invalid=true]:shake aria-[invalid=true]:shadow-[inset_0_0_0_1px_var(--danger-9)] disabled:opacity-50",
						)}
					/>
				</span>
			))}
		</OTPField.Root>
	);
}
