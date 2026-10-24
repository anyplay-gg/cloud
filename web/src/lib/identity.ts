import type { AccentKey, ProfileTheme } from "@/api/types";

// The cloud has no accent or banner theme for a player, so both are derived from the handle:
// the same player looks the same to everyone, on every device, with nothing to store.
const ACCENTS: AccentKey[] = ["blue", "mint", "coral", "violet", "gold", "white"];
const THEMES: ProfileTheme[] = ["cosmic", "aurora", "sunset"];

function hash(text: string): number {
	let h = 2166136261;
	for (const ch of text.toLowerCase()) {
		h ^= ch.charCodeAt(0);
		h = Math.imul(h, 16777619);
	}
	return h >>> 0;
}

export const accentFor = (handle: string): AccentKey =>
	ACCENTS[hash(handle) % ACCENTS.length] ?? "blue";
export const themeFor = (handle: string): ProfileTheme =>
	THEMES[(hash(handle) >>> 3) % THEMES.length] ?? "cosmic";

/** The picture of an avatar hash, served by the cloud on the same origin. */
export const avatarUrl = (hash: string | null | undefined): string | undefined =>
	hash ? `/v1/avatars/${encodeURIComponent(hash)}` : undefined;
