import type { StepUpMethod } from "./types";

/** Every failure the API layer reports, so screens branch on `code` and never on a status. */
export type ApiErrorCode =
	| "network" // the request never got an answer
	| "server" // 5xx
	| "unauthorized" // not signed in, or the session ended
	| "step_up_required" // the action needs a recent sign-in
	| "forbidden"
	| "not_found"
	| "invalid" // the cloud rejected the input; see `fields`
	| "rate_limited"
	| "conflict"
	| "passkey_cancelled" // the person closed the browser's passkey sheet
	| "passkey_unsupported"
	| "unknown";

export class ApiError extends Error {
	readonly code: ApiErrorCode;
	readonly status: number;
	/** Field-level messages from a rejected form, keyed by the input's name. */
	readonly fields: Record<string, string>;
	/** For `step_up_required`: how the player can prove it is them. */
	readonly methods: StepUpMethod[];
	/** For `rate_limited`: seconds to wait. */
	readonly retryAfter: number | null;

	constructor(
		code: ApiErrorCode,
		message: string,
		options: {
			status?: number;
			fields?: Record<string, string>;
			methods?: StepUpMethod[];
			retryAfter?: number | null;
		} = {},
	) {
		super(message);
		this.name = "ApiError";
		this.code = code;
		this.status = options.status ?? 0;
		this.fields = options.fields ?? {};
		this.methods = options.methods ?? [];
		this.retryAfter = options.retryAfter ?? null;
	}
}

export const isApiError = (e: unknown): e is ApiError => e instanceof ApiError;
export const isStepUp = (e: unknown): e is ApiError =>
	isApiError(e) && e.code === "step_up_required";

/** What to show for a failure the screen has no better words for. */
export function describeError(e: unknown): string {
	if (!isApiError(e)) return "Something went wrong. Try again.";
	switch (e.code) {
		case "network":
			return "Can't reach AnyPlay. Check your connection and try again.";
		case "server":
			return "AnyPlay had a problem on its side. Try again in a minute.";
		case "unauthorized":
			return "Your session ended. Sign in again.";
		case "forbidden":
			return "You can't do that with this account.";
		case "not_found":
			return "That doesn't exist any more.";
		case "rate_limited":
			return e.retryAfter
				? `Too many tries. Wait ${e.retryAfter} seconds and try again.`
				: "Too many tries. Wait a moment and try again.";
		case "passkey_unsupported":
			return "This browser can't use passkeys. Use the e-mail code instead.";
		default:
			return e.message || "Something went wrong. Try again.";
	}
}
