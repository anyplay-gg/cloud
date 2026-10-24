import { useCallback, useEffect, useRef, useState } from "react";
import { toast } from "@/ui/toast";

const WINDOW_MS = 6000;

/**
 * "Do it, with a way back." The row disappears at once and a toast offers Undo; only when the
 * window closes does the real request go out. Leaving the screen commits anything pending, so
 * nothing is silently dropped. Use it for actions that are cheap to hold back (signing one
 * device out), never for a change that must happen now.
 */
export function useUndoable<T>(commit: (item: T) => Promise<void>, getId: (item: T) => string) {
	const [hidden, setHidden] = useState<ReadonlySet<string>>(new Set());
	const pending = useRef(new Map<string, { item: T; timer: number }>());
	const commitRef = useRef(commit);
	commitRef.current = commit;

	const settle = useCallback(async (id: string) => {
		const entry = pending.current.get(id);
		if (!entry) return;
		window.clearTimeout(entry.timer);
		pending.current.delete(id);
		try {
			await commitRef.current(entry.item);
		} finally {
			setHidden((h) => {
				const n = new Set(h);
				n.delete(id);
				return n;
			});
		}
	}, []);

	const run = useCallback(
		(item: T, message: string) => {
			const id = getId(item);
			setHidden((h) => new Set(h).add(id));
			const timer = window.setTimeout(() => void settle(id), WINDOW_MS);
			pending.current.set(id, { item, timer });
			toast.undoable(
				message,
				() => {
					const entry = pending.current.get(id);
					if (!entry) return;
					window.clearTimeout(entry.timer);
					pending.current.delete(id);
					setHidden((h) => {
						const n = new Set(h);
						n.delete(id);
						return n;
					});
				},
				WINDOW_MS,
			);
		},
		[getId, settle],
	);

	// leaving the screen commits what is still waiting
	useEffect(() => {
		const map = pending.current;
		return () => {
			for (const id of [...map.keys()]) void settle(id);
		};
	}, [settle]);

	return { hidden, run };
}
