import { Check, Copy } from "lucide-react";
import { AnimatePresence } from "motion/react";
import * as motion from "motion/react-m";
import { useRef, useState } from "react";
import { m } from "@/i18n";
import { cn } from "@/lib/cn";
import { toast } from "@/ui/toast";

/**
 * A value you copy rather than type: an ID, a profile link. Mono text on the left, one button on
 * the right whose icon swaps copy to check (scale, opacity, blur; no bounce). The value stays
 * selectable, and the toast says what happened for people who do not see the icon.
 */
export function CopyField({
	value,
	label,
	className,
}: {
	value: string;
	/** What is being copied, for the button's name and the toast: "Profile link". */
	label: string;
	className?: string;
}) {
	const [copied, setCopied] = useState(false);
	const timer = useRef<number | undefined>(undefined);
	const copy = async () => {
		try {
			await navigator.clipboard.writeText(value);
		} catch {
			const ta = document.createElement("textarea");
			ta.value = value;
			ta.setAttribute("readonly", "");
			ta.style.position = "fixed";
			ta.style.opacity = "0";
			document.body.append(ta);
			ta.select();
			document.execCommand("copy");
			ta.remove();
		}
		setCopied(true);
		toast.success(m.copy_done({ label }));
		window.clearTimeout(timer.current);
		timer.current = window.setTimeout(() => setCopied(false), 1800);
	};
	const spring = { type: "spring", duration: 0.3, bounce: 0 } as const;
	return (
		<div
			className={cn(
				"flex h-9 items-center rounded-md bg-raised ps-3 shadow-[inset_0_0_0_1px_var(--control-border)] coarse:h-11",
				className,
			)}
		>
			<span className="min-w-0 flex-1 select-all truncate font-mono text-small text-fg">
				{value}
			</span>
			<button
				type="button"
				onClick={copy}
				aria-label={m.copy_button({ label })}
				className="press relative flex size-9 shrink-0 items-center justify-center rounded-md text-fg-muted outline-offset-0 pointer-hover:hover:text-fg coarse:size-11"
			>
				<AnimatePresence mode="popLayout" initial={false}>
					<motion.span
						key={copied ? "done" : "copy"}
						initial={{ opacity: 0, scale: 0.25, filter: "blur(4px)" }}
						animate={{ opacity: 1, scale: 1, filter: "blur(0px)" }}
						exit={{ opacity: 0, scale: 0.25, filter: "blur(4px)" }}
						transition={spring}
						className="flex"
					>
						{copied ? (
							<Check aria-hidden className="size-4 text-success-text" strokeWidth={2.25} />
						) : (
							<Copy aria-hidden className="size-4" strokeWidth={1.75} />
						)}
					</motion.span>
				</AnimatePresence>
			</button>
		</div>
	);
}
