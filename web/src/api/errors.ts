import { m } from "@/i18n";
import type { StepUpMethod } from "./types";

/**
 * Every failure the API layer reports, so screens branch on `code` and never on a status. The
 * cloud's own codes (`spec/portal-api.md`) keep their names; the rest are the client's.
 */
export type ApiErrorCode =
	| "network" // the request never got an answer
	| "bad_request"
	| "code_wrong"
	| "code_expired"
	| "handle_invalid"
	| "handle_taken"
	| "unauthorized" // no live session, or a wrong CSRF token
	| "banned"
	| "forbidden"
	| "step_up_required" // the action needs a recent sign-in
	| "not_found"
	| "unsupported_media_type"
	| "limit"
	| "too_large"
	| "rate_limited"
	| "internal"
	| "passkey_cancelled" // the person closed the browser's passkey sheet
	| "passkey_unsupported"
	| "unknown";

const KNOWN: ReadonlySet<string> = new Set<ApiErrorCode>([
	"bad_request",
	"code_wrong",
	"code_expired",
	"handle_invalid",
	"handle_taken",
	"unauthorized",
	"banned",
	"forbidden",
	"step_up_required",
	"not_found",
	"unsupported_media_type",
	"limit",
	"too_large",
	"rate_limited",
	"internal",
]);

export const asErrorCode = (code: unknown, status: number): ApiErrorCode => {
	if (typeof code === "string" && KNOWN.has(code)) return code as ApiErrorCode;
	if (status >= 500) return "internal";
	if (status === 401) return "unauthorized";
	if (status === 404) return "not_found";
	if (status === 429) return "rate_limited";
	return "unknown";
};

export class ApiError extends Error {
	readonly code: ApiErrorCode;
	readonly status: number;
	/** Messages for the input named by the cloud's `field`, keyed by that name. */
	readonly fields: Record<string, string>;
	/** For `step_up_required`: how the player can prove it is them. Filled in by the client. */
	readonly methods: StepUpMethod[];
	/** For `rate_limited`: seconds to wait, from `Retry-After`. */
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
	if (!isApiError(e)) return m.error_generic();
	switch (e.code) {
		case "network":
			return m.error_network();
		case "internal":
			return m.error_server();
		case "unauthorized":
			return m.error_unauthorized();
		case "forbidden":
			return m.error_forbidden();
		case "banned":
			return m.error_banned();
		case "not_found":
			return m.error_not_found();
		case "limit":
			return m.error_limit();
		case "too_large":
			return m.error_too_large();
		case "code_wrong":
			return m.error_code_wrong();
		case "code_expired":
			return m.error_code_expired();
		case "handle_taken":
			return m.error_handle_taken();
		case "handle_invalid":
			return m.auth_handle_rules();
		case "rate_limited":
			return e.retryAfter
				? m.error_rate_limited_in({ seconds: e.retryAfter })
				: m.error_rate_limited();
		case "passkey_unsupported":
			return m.error_passkey_unsupported();
		default:
			return e.message || m.error_generic();
	}
}
