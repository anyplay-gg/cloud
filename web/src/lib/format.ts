// Dates and numbers through Intl, so a second locale needs no changes here.
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

/** "2 hours ago", "yesterday", "just now". */
export function relativeTime(iso: string, now = Date.now()): string {
	const seconds = Math.round((new Date(iso).getTime() - now) / 1000);
	const abs = Math.abs(seconds);
	if (abs < 45) return rtf(getLocale()).format(0, "second");
	for (const [unit, size] of STEPS) {
		if (abs >= size) return rtf(getLocale()).format(Math.round(seconds / size), unit);
	}
	return rtf(getLocale()).format(Math.round(seconds / 60), "minute");
}

export function formatDate(iso: string, style: "long" | "medium" = "long"): string {
	return new Intl.DateTimeFormat(getLocale(), { dateStyle: style }).format(new Date(iso));
}

export function formatMonthYear(iso: string): string {
	return new Intl.DateTimeFormat(getLocale(), { month: "long", year: "numeric" }).format(
		new Date(iso),
	);
}

export const formatNumber = (n: number) => new Intl.NumberFormat(getLocale()).format(n);

/** Whole days from now until an ISO date, never below zero. */
export function daysUntil(iso: string, now = Date.now()): number {
	return Math.max(0, Math.ceil((new Date(iso).getTime() - now) / 86_400_000));
}
