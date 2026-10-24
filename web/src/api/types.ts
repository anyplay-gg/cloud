// The shapes the portal reads and writes. They describe the JSON under /web/api/v1; the doc
// web/docs/api.md says which parts the cloud already serves and which are still to come.
// Timestamps are ISO 8601 strings in UTC.

export type AccentKey = "blue" | "mint" | "coral" | "violet" | "gold" | "white";
export type ProfileTheme = "cosmic" | "aurora" | "sunset";
export type Audience = "everyone" | "friends" | "me";
/** The profile fields a player can show or hide. */
export type ProfileField = "about" | "friends" | "trophies" | "activity";
export type FriendRequestPolicy = "everyone" | "friends_of_friends" | "nobody";
export type PresenceStatus = "online" | "away" | "offline";

export interface Me {
	id: string;
	handle: string;
	displayName: string;
	email: string;
	about: string;
	accent: AccentKey;
	theme: ProfileTheme;
	createdAt: string;
	/** Set while the account waits out its 30-day grace period. */
	deletion: { requestedAt: string; scheduledFor: string } | null;
}

export interface SessionInfo {
	signedIn: boolean;
	/** What the cloud's operator configured for sign-up. */
	terms: { url: string | null; version: string };
	/** Sent back as X-CSRF-Token on every write. */
	csrf: string;
	account: Me | null;
}

export interface Passkey {
	id: string;
	name: string;
	createdAt: string;
	lastUsedAt: string | null;
	/** True when the credential syncs between devices. */
	synced: boolean;
}

export interface DeviceSession {
	id: string;
	kind: "launcher" | "browser";
	/** The player-chosen name for a launcher, the browser and system for a browser. */
	name: string;
	platform: string;
	createdAt: string;
	lastUsedAt: string;
	/** Country only; the cloud never stores a street address for a sign-in. */
	region: string | null;
	current: boolean;
}

export type ActivityKind =
	| "sign_in"
	| "passkey_added"
	| "passkey_removed"
	| "device_signed_out"
	| "signed_out_everywhere"
	| "profile_updated";

export interface ActivityEvent {
	id: string;
	kind: ActivityKind;
	at: string;
	detail: string | null;
}

export interface PrivacySettings {
	fields: Record<ProfileField, Audience>;
	friendRequests: FriendRequestPolicy;
}

export interface ProfileUpdate {
	displayName: string;
	about: string;
	accent: AccentKey;
	theme: ProfileTheme;
}

export interface Person {
	id: string;
	handle: string;
	displayName: string;
	accent: AccentKey;
	theme: ProfileTheme;
}

export interface Friend extends Person {
	status: PresenceStatus;
	/** Free text from the launcher, for example "Playing Tidebreaker". Never art. */
	activity: string | null;
	since: string;
}

export interface FriendRequest {
	id: string;
	person: Person;
	createdAt: string;
}

export interface FriendsOverview {
	friends: Friend[];
	incoming: FriendRequest[];
	outgoing: FriendRequest[];
}

export interface GradeCounts {
	platinum: number;
	gold: number;
	silver: number;
	bronze: number;
}

/** Progress in one title. Only counts: the cloud never sees trophy names, texts or art. */
export interface TitleProgress {
	titleId: string;
	name: string;
	earned: GradeCounts;
	total: GradeCounts;
	/** Whole percent of points unlocked, integer division like the launcher. */
	percent: number;
	lastEarnedAt: string | null;
}

export interface TrophySummary {
	titles: TitleProgress[];
	earned: GradeCounts;
	total: GradeCounts;
	percent: number;
}

export type Relation = "self" | "friend" | "stranger" | "signed_out";

export interface PublicProfile {
	handle: string;
	displayName: string;
	accent: AccentKey;
	theme: ProfileTheme;
	joinedAt: string;
	relation: Relation;
	/** False when the whole profile is private to this viewer. Then only the name is set. */
	visible: boolean;
	about: string | null;
	friendCount: number | null;
	trophies: Omit<TrophySummary, "titles"> | null;
	status: PresenceStatus | null;
	/** Fields the viewer cannot see, so the page can say who could. */
	hidden: { field: ProfileField; audience: Audience }[];
	/** Request state between the viewer and this player. */
	request: "none" | "outgoing" | "incoming";
}

export type SignInIntent = "sign_in" | "create";
export type AuthStep = "code" | "details" | "terms" | "done";

export interface AuthProgress {
	next: AuthStep;
}

export interface CodeChallenge {
	/** The address with the middle hidden, for example "ri***@example.com". */
	emailHint: string;
	/** Seconds until the code can be sent again. */
	resendAfter: number;
}

export interface DataExport {
	filename: string;
	blob: Blob;
}

export type StepUpMethod = "passkey" | "email_code";
