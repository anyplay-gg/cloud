import type {
	ActivityEvent,
	AuthProgress,
	CodeChallenge,
	DataExport,
	DeviceSession,
	FriendsOverview,
	Me,
	Passkey,
	PrivacySettings,
	ProfileUpdate,
	PublicProfile,
	SessionInfo,
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
 */
export interface PortalApi {
	/** "http" talks to a cloud, "mock" is in memory. Screens may show a dev notice for mock. */
	readonly mode: "http" | "mock";

	/** The current browser session. Never rejects for a signed-out visitor. */
	session(): Promise<SessionInfo>;

	auth: {
		/** Sends a sign-in code to the address (or starts account creation). */
		start(input: { email: string; intent: SignInIntent }): Promise<CodeChallenge>;
		resendCode(): Promise<CodeChallenge>;
		verifyCode(code: string): Promise<AuthProgress>;
		submitDetails(input: { handle: string; displayName: string }): Promise<AuthProgress>;
		acceptTerms(): Promise<AuthProgress>;
		/** Whether this browser can do passkeys, and whether the autofill form is available. */
		passkeySupport(): Promise<{ available: boolean; conditional: boolean }>;
		/** Resolves when a passkey signs the player in. With `conditional`, waits for autofill. */
		signInWithPasskey(options: {
			conditional: boolean;
			signal?: AbortSignal;
		}): Promise<AuthProgress>;
		signOut(): Promise<void>;
	};

	account: {
		updateProfile(input: ProfileUpdate): Promise<Me>;
		activity(): Promise<ActivityEvent[]>;
	};

	security: {
		passkeys(): Promise<Passkey[]>;
		addPasskey(name: string): Promise<Passkey>;
		removePasskey(id: string): Promise<void>;
		sessions(): Promise<DeviceSession[]>;
		revokeSession(id: string): Promise<void>;
		/** Ends every session but this browser's. */
		signOutEverywhere(): Promise<void>;
	};

	stepUp: {
		methods(): Promise<StepUpMethod[]>;
		withPasskey(): Promise<void>;
		sendEmailCode(): Promise<CodeChallenge>;
		verifyEmailCode(code: string): Promise<void>;
	};

	privacy: {
		get(): Promise<PrivacySettings>;
		update(input: PrivacySettings): Promise<PrivacySettings>;
	};

	data: {
		exportData(): Promise<DataExport>;
		/** Starts the 30-day grace period. `handle` must match the account's handle. */
		requestDeletion(input: { handle: string }): Promise<Me>;
		cancelDeletion(): Promise<Me>;
	};

	friends: {
		overview(): Promise<FriendsOverview>;
		sendRequest(handle: string): Promise<void>;
		accept(requestId: string): Promise<void>;
		decline(requestId: string): Promise<void>;
		cancel(requestId: string): Promise<void>;
		remove(friendId: string): Promise<void>;
	};

	trophies: {
		summary(): Promise<TrophySummary>;
	};

	profiles: {
		get(handle: string): Promise<PublicProfile>;
	};
}
