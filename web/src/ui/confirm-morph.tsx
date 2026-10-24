import { AnimatePresence } from "motion/react";
import * as motion from "motion/react-m";
import { useEffect, useId, useRef, useState } from "react";
import { m } from "@/i18n";
import { Button } from "./button";

/**
 * A destructive action that asks in place instead of in a dialog (the confirm-morph pattern
 * from Arc UI, written again on Base UI's button). The quiet "Remove" button morphs into
 * "Cancel" and the real verb. Focus lands on Cancel; Escape and a click elsewhere back out.
 * The two states cross-fade with a small blur so they read as one control changing, not two
 * controls swapping.
 */
export function ConfirmMorph({
	label,
	confirmLabel,
	ariaLabel,
	confirmAriaLabel,
	description,
	pending,
	onConfirm,
}: {
	label: string;
	confirmLabel: string;
	ariaLabel: string;
	confirmAriaLabel?: string;
	/** What happens, read out with the confirm button. */
	description?: string;
	pending?: boolean;
	onConfirm: () => void;
}) {
	const descId = useId();
	const [asking, setAsking] = useState(false);
	const root = useRef<HTMLFieldSetElement>(null);
	const cancelRef = useRef<HTMLButtonElement>(null);
	const startRef = useRef<HTMLButtonElement>(null);
	const wasAsking = useRef(false);

	useEffect(() => {
		if (asking) cancelRef.current?.focus();
		else if (wasAsking.current) startRef.current?.focus();
		wasAsking.current = asking;
	}, [asking]);

	useEffect(() => {
		if (!asking || pending) return;
		const away = (e: PointerEvent) => {
			if (!root.current?.contains(e.target as Node)) setAsking(false);
		};
		document.addEventListener("pointerdown", away);
		return () => document.removeEventListener("pointerdown", away);
	}, [asking, pending]);

	const fade = {
		initial: { opacity: 0, filter: "blur(2px)" },
		animate: { opacity: 1, filter: "blur(0px)", transition: { duration: 0.16 } },
		exit: { opacity: 0, filter: "blur(2px)", transition: { duration: 0.1 } },
	};

	return (
		<fieldset
			ref={root}
			className="m-0 min-w-0 border-0 p-0"
			onKeyDown={(e) => {
				if (e.key === "Escape" && asking && !pending) {
					e.stopPropagation();
					setAsking(false);
				}
			}}
		>
			<legend className="sr-only">{ariaLabel}</legend>
			<AnimatePresence mode="wait" initial={false}>
				{asking ? (
					<motion.div key="ask" {...fade} className="flex items-center gap-2">
						<Button
							ref={cancelRef}
							variant="ghost"
							size="sm"
							onClick={() => setAsking(false)}
							disabled={pending}
						>
							{m.common_cancel()}
						</Button>
						<Button
							variant="danger"
							size="sm"
							loading={pending}
							onClick={onConfirm}
							aria-label={confirmAriaLabel}
							aria-describedby={description ? descId : undefined}
						>
							{confirmLabel}
						</Button>
						{description ? (
							<span id={descId} className="sr-only">
								{description}
							</span>
						) : null}
					</motion.div>
				) : (
					<motion.div key="idle" {...fade}>
						<Button
							ref={startRef}
							variant="ghost"
							size="sm"
							onClick={() => setAsking(true)}
							aria-label={ariaLabel}
						>
							{label}
						</Button>
					</motion.div>
				)}
			</AnimatePresence>
		</fieldset>
	);
}
