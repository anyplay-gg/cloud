// Realistic data for the in-memory adapter. Names, handles and titles are invented. Title ids use
// the GAME01234 shape on purpose: real platform title ids never appear in code.
import { addCounts, percentOf } from "@/lib/trophies";
import type {
	AccentKey,
	ActivityEvent,
	Audience,
	DeviceSession,
	Friend,
	FriendRequest,
	GradeCounts,
	Me,
	Passkey,
	Person,
	PresenceStatus,
	PrivacySettings,
	ProfileField,
	ProfileTheme,
	TitleProgress,
	TrophySummary,
} from "../types";

const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;

export const iso = (now: number, agoMs: number) => new Date(now - agoMs).toISOString();
export const ago = { min: MIN, hour: HOUR, day: DAY };

export function seedMe(now: number): Me {
	return {
		id: "acc_riley",
		handle: "riley",
		displayName: "Riley Chen",
		email: "riley.chen@example.com",
		about: "Co-op when I can, solo when I can't. Always up for a rematch.",
		accent: "blue",
		theme: "cosmic",
		createdAt: iso(now, 212 * DAY),
		deletion: null,
	};
}

export function seedPasskeys(now: number): Passkey[] {
	return [
		{
			id: "pk_desktop",
			name: "Desktop PC",
			createdAt: iso(now, 190 * DAY),
			lastUsedAt: iso(now, 2 * HOUR),
			synced: false,
		},
		{
			id: "pk_phone",
			name: "Phone",
			createdAt: iso(now, 143 * DAY),
			lastUsedAt: iso(now, 6 * DAY),
			synced: true,
		},
	];
}

export function seedSessions(now: number): DeviceSession[] {
	return [
		{
			id: "ses_chrome",
			kind: "browser",
			name: "Chrome on Windows",
			platform: "Windows 11",
			createdAt: iso(now, 38 * MIN),
			lastUsedAt: iso(now, 0),
			region: "Germany",
			current: true,
		},
		{
			id: "ses_launcher_desktop",
			kind: "launcher",
			name: "Riley's desktop",
			platform: "AnyPlay Launcher 0.2.0, Windows",
			createdAt: iso(now, 120 * DAY),
			lastUsedAt: iso(now, 2 * HOUR),
			region: "Germany",
			current: false,
		},
		{
			id: "ses_launcher_lounge",
			kind: "launcher",
			name: "Living room",
			platform: "AnyPlay Launcher 0.2.0, Linux",
			createdAt: iso(now, 61 * DAY),
			lastUsedAt: iso(now, 1 * DAY + 3 * HOUR),
			region: "Germany",
			current: false,
		},
		{
			id: "ses_safari_phone",
			kind: "browser",
			name: "Safari on iPhone",
			platform: "iOS 19",
			createdAt: iso(now, 22 * DAY),
			lastUsedAt: iso(now, 5 * DAY),
			region: "Germany",
			current: false,
		},
		{
			id: "ses_firefox_linux",
			kind: "browser",
			name: "Firefox on Linux",
			platform: "Fedora Linux",
			createdAt: iso(now, 80 * DAY),
			lastUsedAt: iso(now, 23 * DAY),
			region: "Netherlands",
			current: false,
		},
	];
}

export function seedActivity(now: number): ActivityEvent[] {
	return [
		{ id: "ev1", kind: "sign_in", at: iso(now, 38 * MIN), detail: "Chrome on Windows" },
		{ id: "ev2", kind: "sign_in", at: iso(now, 2 * HOUR), detail: "Riley's desktop" },
		{ id: "ev3", kind: "profile_updated", at: iso(now, 3 * DAY), detail: null },
		{ id: "ev4", kind: "passkey_added", at: iso(now, 143 * DAY), detail: "Phone" },
		{ id: "ev5", kind: "device_signed_out", at: iso(now, 150 * DAY), detail: "Old laptop" },
	];
}

export const defaultPrivacy = (): PrivacySettings => ({
	fields: { about: "everyone", friends: "friends", trophies: "friends", activity: "friends" },
	friendRequests: "friends_of_friends",
});

const person = (
	id: string,
	handle: string,
	displayName: string,
	accent: AccentKey,
	theme: ProfileTheme,
): Person => ({
	id,
	handle,
	displayName,
	accent,
	theme,
});

export interface MockPlayer extends Person {
	about: string;
	joinedDaysAgo: number;
	status: PresenceStatus;
	activity: string | null;
	friendCount: number;
	fields: Record<ProfileField, Audience>;
	/** A locked profile shows only the name to anyone who is not a friend. */
	locked: boolean;
	trophies: { earned: GradeCounts; total: GradeCounts };
}

const counts = (platinum: number, gold: number, silver: number, bronze: number): GradeCounts => ({
	platinum,
	gold,
	silver,
	bronze,
});

export const players: MockPlayer[] = [
	{
		...person("acc_mira", "mira", "Mira Okonkwo", "mint", "aurora"),
		about: "Speedruns on weekends, cozy games on weeknights.",
		joinedDaysAgo: 330,
		status: "online",
		activity: "Playing Tidebreaker",
		friendCount: 41,
		fields: { about: "everyone", friends: "friends", trophies: "everyone", activity: "friends" },
		locked: false,
		trophies: { earned: counts(3, 21, 44, 96), total: counts(7, 40, 88, 170) },
	},
	{
		...person("acc_jonas", "jonas", "Jonas Albrecht", "coral", "sunset"),
		about: "Racing, mostly. Say hi before you add me.",
		joinedDaysAgo: 96,
		status: "offline",
		activity: null,
		friendCount: 12,
		fields: { about: "everyone", friends: "friends", trophies: "friends", activity: "me" },
		locked: false,
		trophies: { earned: counts(1, 6, 15, 31), total: counts(4, 22, 51, 90) },
	},
	{
		...person("acc_kenji", "kenji", "Kenji Mori", "violet", "cosmic"),
		about: "",
		joinedDaysAgo: 502,
		status: "offline",
		activity: null,
		friendCount: 88,
		fields: { about: "me", friends: "me", trophies: "me", activity: "me" },
		locked: true,
		trophies: { earned: counts(9, 40, 70, 120), total: counts(14, 55, 99, 180) },
	},
	{
		...person("acc_lena", "lena", "Lena Haugen", "gold", "sunset"),
		about: "Collecting every ending.",
		joinedDaysAgo: 410,
		status: "away",
		activity: "Away for 20 minutes",
		friendCount: 27,
		fields: { about: "everyone", friends: "friends", trophies: "friends", activity: "friends" },
		locked: false,
		trophies: { earned: counts(5, 28, 52, 101), total: counts(8, 36, 66, 133) },
	},
	{
		...person("acc_tomas", "tomas", "Tomas Vidal", "white", "cosmic"),
		about: "Fighting games and bad puns.",
		joinedDaysAgo: 60,
		status: "online",
		activity: "In the launcher",
		friendCount: 9,
		fields: { about: "everyone", friends: "friends", trophies: "friends", activity: "friends" },
		locked: false,
		trophies: { earned: counts(0, 3, 9, 18), total: counts(2, 12, 30, 70) },
	},
	{
		...person("acc_sasha", "sasha", "Sasha Petrov", "blue", "aurora"),
		about: "Soulslikes, strategy and tea.",
		joinedDaysAgo: 250,
		status: "offline",
		activity: null,
		friendCount: 33,
		fields: { about: "everyone", friends: "friends", trophies: "friends", activity: "friends" },
		locked: false,
		trophies: { earned: counts(2, 14, 30, 61), total: counts(5, 30, 60, 112) },
	},
	{
		...person("acc_ines", "ines", "Ines Duarte", "mint", "aurora"),
		about: "Puzzle games.",
		joinedDaysAgo: 14,
		status: "offline",
		activity: null,
		friendCount: 4,
		fields: { about: "everyone", friends: "friends", trophies: "friends", activity: "friends" },
		locked: false,
		trophies: { earned: counts(0, 1, 4, 9), total: counts(1, 8, 18, 40) },
	},
];

export const toPerson = (p: Person): Person => ({
	id: p.id,
	handle: p.handle,
	displayName: p.displayName,
	accent: p.accent,
	theme: p.theme,
});

export function seedFriends(now: number): Friend[] {
	const friendIds = ["acc_mira", "acc_lena", "acc_tomas", "acc_sasha"];
	const since = [180, 301, 40, 122];
	return friendIds.map((id, i) => {
		const p = players.find((x) => x.id === id) as MockPlayer;
		return {
			...toPerson(p),
			status: p.status,
			activity: p.activity,
			since: iso(now, (since[i] ?? 10) * DAY),
		};
	});
}

export function seedIncoming(now: number): FriendRequest[] {
	const p = players.find((x) => x.id === "acc_jonas") as MockPlayer;
	return [{ id: "fr_in_jonas", person: toPerson(p), createdAt: iso(now, 2 * DAY) }];
}

export function seedOutgoing(now: number): FriendRequest[] {
	const p = players.find((x) => x.id === "acc_ines") as MockPlayer;
	return [{ id: "fr_out_ines", person: toPerson(p), createdAt: iso(now, 5 * HOUR) }];
}

// Invented titles. The cloud holds counts per title and nothing else about a trophy.
const titles: {
	titleId: string;
	name: string;
	earned: GradeCounts;
	total: GradeCounts;
	ago: number | null;
}[] = [
	{
		titleId: "GAME01234",
		name: "Tidebreaker",
		earned: counts(1, 9, 14, 22),
		total: counts(1, 12, 20, 30),
		ago: 3 * HOUR,
	},
	{
		titleId: "GAME02417",
		name: "Hollow Parade",
		earned: counts(0, 4, 11, 19),
		total: counts(1, 10, 18, 28),
		ago: 2 * DAY,
	},
	{
		titleId: "GAME03102",
		name: "Night Courier",
		earned: counts(0, 2, 6, 10),
		total: counts(1, 7, 14, 24),
		ago: 9 * DAY,
	},
	{
		titleId: "GAME04550",
		name: "Kiln & Ember",
		earned: counts(1, 6, 8, 15),
		total: counts(1, 6, 8, 15),
		ago: 31 * DAY,
	},
	{
		titleId: "GAME05871",
		name: "Paper Frontier",
		earned: counts(0, 0, 2, 5),
		total: counts(1, 8, 16, 26),
		ago: 47 * DAY,
	},
	{
		titleId: "GAME06623",
		name: "Lowtide Radio",
		earned: counts(0, 0, 0, 0),
		total: counts(1, 5, 10, 18),
		ago: null,
	},
];

export function seedTrophies(now: number): TrophySummary {
	const list: TitleProgress[] = titles.map((t) => ({
		titleId: t.titleId,
		name: t.name,
		earned: t.earned,
		total: t.total,
		percent: percentOf(t.earned, t.total),
		lastEarnedAt: t.ago === null ? null : iso(now, t.ago),
	}));
	const earned = list.reduce((a, t) => addCounts(a, t.earned), counts(0, 0, 0, 0));
	const total = list.reduce((a, t) => addCounts(a, t.total), counts(0, 0, 0, 0));
	return { titles: list, earned, total, percent: percentOf(earned, total) };
}
