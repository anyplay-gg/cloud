import { useEffect, useRef, useState } from "react";
import type { SaveState } from "@/ui/save-status";

/** idle -> saving -> saved, and back to idle after a moment. Failures call `fail`. */
export function useSaveState(resetAfterMs = 2200) {
	const [state, setState] = useState<SaveState>("idle");
	const timer = useRef<number | undefined>(undefined);
	useEffect(() => () => window.clearTimeout(timer.current), []);
	return {
		state,
		saving: () => {
			window.clearTimeout(timer.current);
			setState("saving");
		},
		saved: () => {
			setState("saved");
			window.clearTimeout(timer.current);
			timer.current = window.setTimeout(() => setState("idle"), resetAfterMs);
		},
		fail: () => {
			window.clearTimeout(timer.current);
			setState("idle");
		},
	};
}
