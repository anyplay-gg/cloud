import {
	createContext,
	type ReactNode,
	useCallback,
	useContext,
	useEffect,
	useMemo,
	useState,
} from "react";
import type { AccentKey } from "@/api/types";

export type ThemeChoice = "system" | "dark" | "light";
export const ACCENT_KEYS: readonly AccentKey[] = [
	"blue",
	"mint",
	"coral",
	"violet",
	"gold",
	"white",
];
const DEFAULT_ACCENT: AccentKey = "blue";

const THEME_KEY = "anyplay.theme";
const ACCENT_KEY = "anyplay.accent";

function read(key: string): string | null {
	try {
		return localStorage.getItem(key);
	} catch {
		return null;
	}
}
function write(key: string, value: string | null) {
	try {
		if (value === null) localStorage.removeItem(key);
		else localStorage.setItem(key, value);
	} catch {
		// private window or blocked storage: the choice lasts for this page load
	}
}

function initialTheme(): ThemeChoice {
	const v = read(THEME_KEY);
	return v === "dark" || v === "light" ? v : "system";
}
function initialAccent(): AccentKey {
	const v = read(ACCENT_KEY);
	return ACCENT_KEYS.includes(v as AccentKey) ? (v as AccentKey) : DEFAULT_ACCENT;
}

/**
 * A theme flip changes colour on nearly every element at once. With transitions running, the
 * switch smears. Turn them off, force a reflow, and turn them back on in the next frame.
 */
function withoutTransitions(change: () => void) {
	const root = document.documentElement;
	root.classList.add("no-transitions");
	change();
	void getComputedStyle(root).opacity;
	requestAnimationFrame(() => requestAnimationFrame(() => root.classList.remove("no-transitions")));
}

function applyTheme(t: ThemeChoice) {
	const root = document.documentElement;
	if (t === "system") root.removeAttribute("data-theme");
	else root.setAttribute("data-theme", t);
}
function applyAccent(a: AccentKey) {
	const root = document.documentElement;
	if (a === DEFAULT_ACCENT) root.removeAttribute("data-accent");
	else root.setAttribute("data-accent", a);
}

interface Appearance {
	theme: ThemeChoice;
	accent: AccentKey;
	setTheme: (t: ThemeChoice) => void;
	setAccent: (a: AccentKey) => void;
}

const Ctx = createContext<Appearance | null>(null);

export function AppearanceProvider({ children }: { children: ReactNode }) {
	const [theme, setThemeState] = useState<ThemeChoice>(initialTheme);
	const [accent, setAccentState] = useState<AccentKey>(initialAccent);

	// Keeps the attributes right on first render (theme-init.js already set them before paint).
	useEffect(() => {
		applyTheme(theme);
		applyAccent(accent);
	}, [theme, accent]);

	const setTheme = useCallback((t: ThemeChoice) => {
		withoutTransitions(() => {
			applyTheme(t);
			setThemeState(t);
		});
		write(THEME_KEY, t === "system" ? null : t);
	}, []);
	const setAccent = useCallback((a: AccentKey) => {
		withoutTransitions(() => {
			applyAccent(a);
			setAccentState(a);
		});
		write(ACCENT_KEY, a === DEFAULT_ACCENT ? null : a);
	}, []);

	const value = useMemo(
		() => ({ theme, accent, setTheme, setAccent }),
		[theme, accent, setTheme, setAccent],
	);
	return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useAppearance(): Appearance {
	const v = useContext(Ctx);
	if (!v) throw new Error("useAppearance needs <AppearanceProvider>");
	return v;
}
