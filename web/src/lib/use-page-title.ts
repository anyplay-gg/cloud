import { useEffect } from "react";

/** One document title per screen, so a screen reader and the tab strip both say where you are. */
export function usePageTitle(title: string) {
	useEffect(() => {
		document.title = title ? `${title} · AnyPlay` : "AnyPlay";
	}, [title]);
}
