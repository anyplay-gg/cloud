// The shapes the portal reads and writes, as `docs/api.md` and the network's `spec/portal-api.md`
// describe them under /web/api/v1. Times are Unix seconds; account and device ids are integers.
// Keys are camelCase here and snake_case on the wire (`case.ts`).

export type AccentKey = "blue" | "mint" | "coral" | "violet" | "gold" | "white";
export type ProfileTheme = "cosmic" | "aurora" | "sunset";

/** Who sees a part of a profile. `nobody` means only the player. */
export type Audience = "everyone" | "friends" | "nobody";
export type ProfilePart = "trophies" | "about" | "favourites" | "avatar" | "playtime";
export type Privacy = Record<ProfilePart, Audience>;

export interface Account {
	id: number;
	handle: string;
}

export interface Me extends Account {
	email: string;
	created: number;
}

export interface CloudInfo {
	name: string;
	/** "open" or "closed": a closed cloud makes no new accounts. */
	signup: string;
	privacyUrl: string | null;
	termsUrl: string | null;
}

export type FlowStep = "email" | "code" | "details" | "terms" | "banned" | "handoff" | "done";

export interface Flow {
	step: FlowStep;
	email?: string;
	/** The player wants a new account. */
	create?: boolean;
	/** For `handoff`: the loopback address of the launcher, with its one-time code. */
	redirect?: string;
	terms?: { version: string; url: string | null };
}

/** What `GET /session` and every sign-in step answer. */
export interface SessionView {
	/** Sent back as X-CSRF-Token on every write; null once the session is over. */
	csrf: string | null;
	signedIn: boolean;
	account: Me | null;
	freshUntil: number | null;
	flow: Flow;
	/** Set while the launcher on the player's PC waits for this sign-in. */
	authorize: { device: string; create: boolean } | null;
	/** A line to show once. */
	notice: string | null;
	cloud: CloudInfo;
}

export interface Passkey {
	/** The credential id, base64url. */
	id: string;
	name: string;
	created: number;
	lastUsed: number | null;
}

/** A launcher. */
export interface Device {
	id: number;
	name: string;
	platform: string;
	created: number;
	lastUsed: number;
}

/** A browser session; the cloud stores nothing about the browser itself. */
export interface BrowserSession {
	id: number;
	current: boolean;
	created: number;
	lastUsed: number;
}

export interface AccountView {
	account: Me;
	passkeys: Passkey[];
	maxPasskeys: number;
	devices: Device[];
	sessions: BrowserSession[];
	/** Set while the account waits out its 30-day grace period. */
	deletion: { at: number } | null;
	/** A new address waiting for its code. */
	pendingEmail: string | null;
	freshUntil: number | null;
}

export interface Profile {
	account: Account;
	about: string | null;
	/** Title ids, at most three. */
	favourites: string[] | null;
	/** The hash of the picture; the file is at /v1/avatars/{hash}. */
	avatar: string | null;
	playtime: { titleId: string; minutes: number }[] | null;
}

export interface ProfileUpdate {
	about?: string;
	favourites?: string[];
}

export interface Activity {
	titleId: string;
	title: string;
}

export interface Presence {
	online: boolean;
	activity: Activity | null;
}

export interface Friend {
	account: Account;
	since: number;
	presence: Presence;
}

export interface FriendRequest {
	account: Account;
	since: number;
}

export interface FriendLists {
	friends: Friend[];
	incoming: FriendRequest[];
	outgoing: FriendRequest[];
	blocked: Account[];
}

/** Trophies earned per grade. Counts only: the cloud never has names, texts or art. */
export interface GradeCounts {
	platinum: number;
	gold: number;
	silver: number;
	bronze: number;
}

export interface TitleSummary {
	titleId: string;
	counts: GradeCounts;
	earned: number;
	lastEarned: number | null;
}

export interface TrophySummary {
	counts: GradeCounts;
	titles: TitleSummary[];
}

export interface Viewer {
	relation: "self" | "friend" | "none";
	requested: "incoming" | "outgoing" | null;
	blocked: boolean;
}

export interface PublicUser {
	account: Account;
	about: string | null;
	favourites: string[] | null;
	avatar: string | null;
	playtime: { titleId: string; minutes: number }[] | null;
	trophies: GradeCounts | null;
	/** Null for a visitor who is not signed in. */
	viewer: Viewer | null;
}

export type Appeal =
	| { state: "none" }
	| { state: "open"; deadline: number }
	| { state: "filed"; status: "open" | "upheld" | "reversed"; answer: string | null }
	| { state: "over" };

export interface BanStatement {
	until: number | null;
	rule: string;
	reason: string;
	decided: number;
	appeal: Appeal;
}

export type SignInIntent = "sign_in" | "create";

export interface DataExport {
	filename: string;
	blob: Blob;
}

export type StepUpMethod = "passkey" | "email_code";
