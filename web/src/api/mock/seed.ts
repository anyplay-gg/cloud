// Realistic data for the in-memory adapter. Names, handles and titles are invented. Title ids use
// the GAME01234 shape on purpose: real platform title ids never appear in code. Times are Unix
// seconds, ids are integers, as on the wire.
import type {
	Account,
	BrowserSession,
	Device,
	Friend,
	FriendRequest,
	GradeCounts,
	Me,
	Passkey,
	Presence,
	Privacy,
	TitleSummary,
	TrophySummary,
} from "../types";

const MIN = 60;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;
export const ago = { min: MIN, hour: HOUR, day: DAY };

export function seedMe(now: number): Me {
	return { id: 41, handle: "riley", email: "riley.chen@example.com", created: now - 212 * DAY };
}

export function seedPasskeys(now: number): Passkey[] {
	return [
		{ id: "pk-desktop", name: "Desktop PC", created: now - 190 * DAY, lastUsed: now - 2 * HOUR },
		{ id: "pk-phone", name: "Phone", created: now - 143 * DAY, lastUsed: now - 6 * DAY },
	];
}

export function seedDevices(now: number): Device[] {
	return [
		{
			id: 3,
			name: "Riley's desktop",
			platform: "windows",
			created: now - 120 * DAY,
			lastUsed: now - 2 * HOUR,
		},
		{
			id: 7,
			name: "Living room",
			platform: "linux",
			created: now - 61 * DAY,
			lastUsed: now - 1 * DAY - 3 * HOUR,
		},
	];
}

export function seedSessions(now: number): BrowserSession[] {
	return [
		{ id: 11, current: true, created: now - 38 * MIN, lastUsed: now },
		{ id: 12, current: false, created: now - 22 * DAY, lastUsed: now - 5 * DAY },
		{ id: 13, current: false, created: now - 80 * DAY, lastUsed: now - 23 * DAY },
	];
}

export const defaultPrivacy = (): Privacy => ({
	trophies: "friends",
	about: "friends",
	favourites: "friends",
	avatar: "friends",
	playtime: "friends",
});

export interface MockPlayer extends Account {
	about: string;
	favourites: string[];
	playtime: { titleId: string; minutes: number }[];
	joinedDaysAgo: number;
	online: boolean;
	activity: { titleId: string; title: string } | null;
	privacy: Privacy;
	trophies: GradeCounts;
	avatar: string | null;
}

const counts = (platinum: number, gold: number, silver: number, bronze: number): GradeCounts => ({
	platinum,
	gold,
	silver,
	bronze,
});
const everyone = (): Privacy => ({
	trophies: "everyone",
	about: "everyone",
	favourites: "everyone",
	avatar: "everyone",
	playtime: "friends",
});

export const players: MockPlayer[] = [
	{
		id: 101,
		handle: "mira",
		about: "Speedruns on weekends, cozy games on weeknights.",
		favourites: ["GAME01234", "GAME04550"],
		playtime: [
			{ titleId: "GAME01234", minutes: 5120 },
			{ titleId: "GAME04550", minutes: 1840 },
		],
		joinedDaysAgo: 330,
		online: true,
		activity: { titleId: "GAME01234", title: "Tidebreaker" },
		privacy: everyone(),
		trophies: counts(3, 21, 44, 96),
		avatar: null,
	},
	{
		id: 102,
		handle: "jonas",
		about: "Racing, mostly. Say hi before you add me.",
		favourites: ["GAME03102"],
		playtime: [],
		joinedDaysAgo: 96,
		online: false,
		activity: null,
		privacy: { ...defaultPrivacy(), about: "everyone", favourites: "everyone" },
		trophies: counts(1, 6, 15, 31),
		avatar: null,
	},
	{
		id: 103,
		handle: "kenji",
		about: "",
		favourites: [],
		playtime: [],
		joinedDaysAgo: 502,
		online: false,
		activity: null,
		privacy: {
			trophies: "nobody",
			about: "nobody",
			favourites: "nobody",
			avatar: "nobody",
			playtime: "nobody",
		},
		trophies: counts(9, 40, 70, 120),
		avatar: null,
	},
	{
		id: 104,
		handle: "lena",
		about: "Collecting every ending.",
		favourites: ["GAME02417"],
		playtime: [{ titleId: "GAME02417", minutes: 3300 }],
		joinedDaysAgo: 410,
		online: true,
		activity: null,
		privacy: defaultPrivacy(),
		trophies: counts(5, 28, 52, 101),
		avatar: null,
	},
	{
		id: 105,
		handle: "tomas",
		about: "Fighting games and bad puns.",
		favourites: [],
		playtime: [],
		joinedDaysAgo: 60,
		online: true,
		activity: { titleId: "GAME05871", title: "Paper Frontier" },
		privacy: defaultPrivacy(),
		trophies: counts(0, 3, 9, 18),
		avatar: null,
	},
	{
		id: 106,
		handle: "sasha",
		about: "Soulslikes, strategy and tea.",
		favourites: ["GAME04550"],
		playtime: [],
		joinedDaysAgo: 250,
		online: false,
		activity: null,
		privacy: defaultPrivacy(),
		trophies: counts(2, 14, 30, 61),
		avatar: null,
	},
	{
		id: 107,
		handle: "ines",
		about: "Puzzle games.",
		favourites: [],
		playtime: [],
		joinedDaysAgo: 14,
		online: false,
		activity: null,
		privacy: everyone(),
		trophies: counts(0, 1, 4, 9),
		avatar: null,
	},
];

export const toAccount = (p: Account): Account => ({ id: p.id, handle: p.handle });
const presenceOf = (p: MockPlayer): Presence => ({ online: p.online, activity: p.activity });
export const presenceFor = presenceOf;

export function seedFriends(now: number): Friend[] {
	const since = [180, 301, 40, 122];
	return [101, 104, 105, 106].map((id, i) => {
		const p = players.find((x) => x.id === id) as MockPlayer;
		return { account: toAccount(p), since: now - (since[i] ?? 10) * DAY, presence: presenceOf(p) };
	});
}

export function seedIncoming(now: number): FriendRequest[] {
	return [
		{ account: toAccount(players.find((x) => x.id === 102) as MockPlayer), since: now - 2 * DAY },
	];
}

export function seedOutgoing(now: number): FriendRequest[] {
	return [
		{ account: toAccount(players.find((x) => x.id === 107) as MockPlayer), since: now - 5 * HOUR },
	];
}

// Invented titles. The cloud holds a title id and counts, and nothing else about a trophy.
const titles: { titleId: string; counts: GradeCounts; ago: number }[] = [
	{ titleId: "GAME01234", counts: counts(1, 9, 14, 22), ago: 3 * HOUR },
	{ titleId: "GAME02417", counts: counts(0, 4, 11, 19), ago: 2 * DAY },
	{ titleId: "GAME03102", counts: counts(0, 2, 6, 10), ago: 9 * DAY },
	{ titleId: "GAME04550", counts: counts(1, 6, 8, 15), ago: 31 * DAY },
	{ titleId: "GAME05871", counts: counts(0, 0, 2, 5), ago: 47 * DAY },
];

export function seedTrophies(now: number): TrophySummary {
	const list: TitleSummary[] = titles.map((t) => ({
		titleId: t.titleId,
		counts: t.counts,
		earned: t.counts.platinum + t.counts.gold + t.counts.silver + t.counts.bronze,
		lastEarned: now - t.ago,
	}));
	const total = list.reduce(
		(a, t) => ({
			platinum: a.platinum + t.counts.platinum,
			gold: a.gold + t.counts.gold,
			silver: a.silver + t.counts.silver,
			bronze: a.bronze + t.counts.bronze,
		}),
		counts(0, 0, 0, 0),
	);
	return { counts: total, titles: list };
}

export const ownPlaytime = [
	{ titleId: "GAME01234", minutes: 7260 },
	{ titleId: "GAME02417", minutes: 2430 },
	{ titleId: "GAME04550", minutes: 960 },
];
