import type {
	AccountView,
	BanStatement,
	DataExport,
	FriendLists,
	Me,
	Passkey,
	Privacy,
	Profile,
	ProfileUpdate,
	PublicUser,
	SessionView,
	SignInIntent,
	StepUpMethod,
	TrophySummary,
} from "./types";

/**
 * The one seam between the screens and the cloud.
 *
 * Two adapters satisfy it: `createHttpApi` talks to /web/api/v1 on the same origin, and
 * `createMockApi` keeps everything in memory so the app runs and can be photographed without a
 * backend. Screens import this interface and the hooks in ./queries, never an adapter.
 *
 * Invariants every adapter keeps:
 * - Failures are `ApiError`s; no adapter throws anything else.
 * - A write that needs a recent sign-in rejects with code `step_up_required`. The screen runs
 *   the step-up dialog (`useStepUp().run`) and the dialog calls the same write again.
 * - Passkey ceremonies happen inside the adapter. The screen calls `addPasskey()` and gets a
 *   `Passkey` back, or `passkey_cancelled` when the person closed the browser's sheet.
 * - Every sign-in step answers with the whole `SessionView`; the screen shows `flow.step`.
 */
export interface PortalApi {
	/** "http" talks to a cloud, "mock" is in memory. */
	readonly mode: "http" | "mock";

	/** The browser's session. Starts one when there is none; never rejects for a signed-out visitor. */
	session(): Promise<SessionView>;

	auth: {
		start(input: { email: string; intent: SignInIntent }): Promise<SessionView>;
		resendCode(): Promise<SessionView>;
		verifyCode(code: string): Promise<SessionView>;
		submitDetails(input: {
			handle: string;
			ageConfirmed: boolean;
			termsAccepted: boolean;
		}): Promise<SessionView>;
		acceptTerms(): Promise<SessionView>;
		/** Whether this browser can do passkeys, and whether the autofill form is available. */
		passkeySupport(): Promise<{ available: boolean; conditional: boolean }>;
		/** Resolves when a passkey signs the player in. With `conditional`, waits for autofill. */
		signInWithPasskey(options: {
			conditional: boolean;
			signal?: AbortSignal;
		}): Promise<SessionView>;
		signOut(): Promise<void>;
		/** The statement of the ban this session proved to be its own. */
		ban(): Promise<BanStatement>;
		appeal(text: string): Promise<void>;
	};

	account: {
		get(): Promise<AccountView>;
		/** Sends a code to the new address. Needs a recent sign-in. */
		changeEmail(email: string): Promise<void>;
		confirmEmail(code: string): Promise<string>;
		profile(): Promise<Profile>;
		updateProfile(input: ProfileUpdate): Promise<Profile>;
		setAvatar(file: Blob): Promise<string>;
		removeAvatar(): Promise<void>;
		privacy(): Promise<Privacy>;
		updatePrivacy(input: Privacy): Promise<Privacy>;
	};

	security: {
		addPasskey(name: string): Promise<Passkey>;
		renamePasskey(id: string, name: string): Promise<void>;
		removePasskey(id: string): Promise<void>;
		removeDevice(id: number): Promise<void>;
		/** Ends a browser session. `signedOut` is true when it was this browser's own. */
		revokeSession(id: number): Promise<{ signedOut: boolean }>;
		/** Ends every device and browser, this one too. */
		signOutEverywhere(): Promise<void>;
	};

	stepUp: {
		methods(): Promise<StepUpMethod[]>;
		withPasskey(): Promise<void>;
		/** Sends a code to the account's own address; answers the hint, "a***@example.com". */
		sendEmailCode(): Promise<string>;
		verifyEmailCode(code: string): Promise<void>;
	};

	data: {
		exportData(): Promise<DataExport>;
		/** Starts the 30-day grace period. `handle` must match the account's handle. */
		requestDeletion(handle: string): Promise<{ at: number }>;
		cancelDeletion(): Promise<void>;
	};

	friends: {
		lists(): Promise<FriendLists>;
		sendRequest(handle: string): Promise<"requested" | "friends">;
		accept(accountId: number): Promise<void>;
		/** Declines an incoming request or withdraws an outgoing one. */
		dropRequest(accountId: number): Promise<void>;
		remove(accountId: number): Promise<void>;
		block(accountId: number): Promise<void>;
		unblock(accountId: number): Promise<void>;
	};

	trophies: {
		mine(): Promise<TrophySummary>;
	};

	users: {
		get(handle: string): Promise<PublicUser>;
	};
}

export type { Me };
