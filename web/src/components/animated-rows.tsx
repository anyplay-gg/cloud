import { AnimatePresence } from "motion/react";
import * as motion from "motion/react-m";
import type { ReactNode } from "react";
import { cn } from "@/lib/cn";

const EASE_OUT = [0.23, 1, 0.32, 1] as const;

/**
 * A divided list whose rows ease in and out. Rows are added or removed a few times a session
 * (a passkey, a request, a device), so this is the one place list motion is worth having: it
 * stops the rows below from teleporting. Height and opacity only, under 200ms, off for reduced
 * motion (Motion drops the height travel and keeps the fade).
 */
export function AnimatedRows({
	children,
	label,
	className,
}: {
	children: ReactNode;
	label?: string;
	className?: string;
}) {
	return (
		<ul aria-label={label} className={className}>
			<AnimatePresence initial={false}>{children}</AnimatePresence>
		</ul>
	);
}

export function AnimatedRow({ children, className }: { children: ReactNode; className?: string }) {
	return (
		<motion.li
			initial={{ opacity: 0, height: 0 }}
			animate={{ opacity: 1, height: "auto", transition: { duration: 0.2, ease: EASE_OUT } }}
			exit={{ opacity: 0, height: 0, transition: { duration: 0.16, ease: EASE_OUT } }}
			className={cn("overflow-hidden border-line [&:not(:first-child)]:border-t", className)}
		>
			{children}
		</motion.li>
	);
}
