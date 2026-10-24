import { Check, Loader2 } from "lucide-react";
import { AnimatePresence } from "motion/react";
import * as motion from "motion/react-m";
import { m } from "@/i18n";

export type SaveState = "idle" | "saving" | "saved";

/**
 * A line of status next to the control that was saved, so confirmation sits where the eye is.
 * The icon swaps with opacity, scale and blur; the words change with it, so motion is never the
 * only signal. It reserves its space, so nothing shifts when it appears.
 */
export function SaveStatus({ state }: { state: SaveState }) {
	return (
		<span role="status" aria-live="polite" className="flex h-5 min-w-24 items-center gap-1.5 text-small text-fg-muted">
			<AnimatePresence mode="popLayout" initial={false}>
				{state === "saving" ? (
					<motion.span
						key="saving"
						initial={{ opacity: 0, scale: 0.25, filter: "blur(4px)" }}
						animate={{ opacity: 1, scale: 1, filter: "blur(0px)" }}
						exit={{ opacity: 0, scale: 0.25, filter: "blur(4px)" }}
						transition={{ type: "spring", duration: 0.3, bounce: 0 }}
						className="flex items-center gap-1.5"
					>
						<Loader2 aria-hidden className="size-3.5 animate-spin" />
						{m.status_saving()}
					</motion.span>
				) : state === "saved" ? (
					<motion.span
						key="saved"
						initial={{ opacity: 0, scale: 0.25, filter: "blur(4px)" }}
						animate={{ opacity: 1, scale: 1, filter: "blur(0px)" }}
						exit={{ opacity: 0, scale: 0.25, filter: "blur(4px)" }}
						transition={{ type: "spring", duration: 0.3, bounce: 0 }}
						className="flex items-center gap-1.5 text-success-text"
					>
						<Check aria-hidden strokeWidth={2.5} className="size-3.5" />
						{m.common_saved()}
					</motion.span>
				) : null}
			</AnimatePresence>
		</span>
	);
}
