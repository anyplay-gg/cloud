import type { LucideIcon } from "lucide-react";
import {
	type MotionValue,
	useMotionValue,
	useReducedMotion,
	useSpring,
	useTransform,
} from "motion/react";
import * as motion from "motion/react-m";
import { useRef } from "react";
import { cn } from "@/lib/cn";
import { Tooltip } from "@/ui/tooltip";

export interface DockLink {
	label: string;
	href: string;
	icon: LucideIcon;
	external?: boolean;
}

/**
 * A row of links whose icons swell toward the pointer (the magnetic dock pattern from
 * Componentry, written again on our tokens). Only a fine pointer drives it: touch, keyboard and
 * reduced motion get a plain row, and every item stays a real link with a name.
 */
export function MagneticDock({ links, label }: { links: DockLink[]; label: string }) {
	const pointerX = useMotionValue(Number.POSITIVE_INFINITY);
	const reduce = useReducedMotion();
	return (
		<nav aria-label={label}>
			<ul
				className="flex w-fit items-end gap-2 rounded-2xl bg-[color-mix(in_oklab,var(--bg)_55%,transparent)] p-2 shadow-[0_0_0_1px_var(--line-strong)] backdrop-blur-md"
				onPointerMove={(e) => {
					if (!reduce && e.pointerType === "mouse") pointerX.set(e.clientX);
				}}
				onPointerLeave={() => pointerX.set(Number.POSITIVE_INFINITY)}
			>
				{links.map((l) => (
					<DockItem key={l.href} link={l} pointerX={pointerX} />
				))}
			</ul>
		</nav>
	);
}

function DockItem({ link, pointerX }: { link: DockLink; pointerX: MotionValue<number> }) {
	const ref = useRef<HTMLAnchorElement>(null);
	const distance = useTransform(pointerX, (x) => {
		const b = ref.current?.getBoundingClientRect();
		return b ? x - (b.left + b.width / 2) : Number.POSITIVE_INFINITY;
	});
	const target = useTransform(distance, [-120, 0, 120], [1, 1.32, 1]);
	const scale = useSpring(target, { mass: 0.1, stiffness: 190, damping: 14 });
	const Icon = link.icon;
	return (
		<li>
			<Tooltip label={link.label} side="bottom">
				<motion.a
					ref={ref}
					href={link.href}
					style={{ scale, transformOrigin: "50% 100%" }}
					{...(link.external ? { target: "_blank", rel: "noopener noreferrer" } : {})}
					aria-label={link.external ? `${link.label} (opens in a new tab)` : link.label}
					className={cn(
						"flex size-11 items-center justify-center rounded-xl bg-raised text-fg-muted outline-offset-2",
						"shadow-[inset_0_0_0_1px_var(--line-strong)] transition-colors duration-[var(--dur-quick)] pointer-hover:hover:text-fg",
					)}
				>
					<Icon aria-hidden strokeWidth={1.75} className="size-5" />
				</motion.a>
			</Tooltip>
		</li>
	);
}
