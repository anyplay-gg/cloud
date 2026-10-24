// Dates and numbers through Intl, so a second locale needs no changes here. Times are Unix
// seconds, as the cloud sends them.
import { getLocale } from "@/i18n";

const rtfCache = new Map<string, Intl.RelativeTimeFormat>();
function rtf(locale: string) {
	let f = rtfCache.get(locale);
	if (!f) {
		f = new Intl.RelativeTimeFormat(locale, { numeric: "auto", style: "long" });
		rtfCache.set(locale, f);
	}
	return f;
}

const STEPS: [Intl.RelativeTimeFormatUnit, number][] = [
	["year", 365 * 86_400],
	["month", 30 * 86_400],
	["week", 7 * 86_400],
	["day", 86_400],
	["hour", 3_600],
	["minute", 60],
];

const nowSeconds = () => Math.floor(Date.now() / 1000);

/** "2 hours ago", "yesterday", "just now". */
export function relativeTime(seconds: number, now = nowSeconds()): string {
	const diff = Math.round(seconds - now);
	const abs = Math.abs(diff);
	if (abs < 45) return rtf(getLocale()).format(0, "second");
	for (const [unit, size] of STEPS) {
		if (abs >= size) return rtf(getLocale()).format(Math.round(diff / size), unit);
	}
	return rtf(getLocale()).format(Math.round(diff / 60), "minute");
}

export function formatDate(seconds: number, style: "long" | "medium" = "long"): string {
	return new Intl.DateTimeFormat(getLocale(), { dateStyle: style }).format(
		new Date(seconds * 1000),
	);
}

export function formatMonthYear(seconds: number): string {
	return new Intl.DateTimeFormat(getLocale(), { month: "long", year: "numeric" }).format(
		new Date(seconds * 1000),
	);
}

export const formatNumber = (n: number) => new Intl.NumberFormat(getLocale()).format(n);

/** Whole days from now until a time, never below zero. */
export function daysUntil(seconds: number, now = nowSeconds()): number {
	return Math.max(0, Math.ceil((seconds - now) / 86_400));
}

/** 7260 minutes as "121 h", 45 as "45 min". */
export function formatPlaytime(minutes: number): string {
	if (minutes < 60) return `${minutes} min`;
	return `${formatNumber(Math.round(minutes / 60))} h`;
}
