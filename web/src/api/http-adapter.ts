import { camelize, snakeize } from "./case";
import { ApiError, type ApiErrorCode } from "./errors";
import type { PortalApi } from "./portal-api";
import type {
	ActivityEvent,
	AuthProgress,
	CodeChallenge,
	DeviceSession,
	FriendsOverview,
	Me,
	Passkey,
	PrivacySettings,
	PublicProfile,
	SessionInfo,
	StepUpMethod,
	TrophySummary,
} from "./types";
import {
	type CredentialJSON,
	createCredential,
	getAssertion,
	passkeySupport,
	type RequestOptionsJSON,
} from "./webauthn";

export interface HttpApiOptions {
	/** Defaults to the same origin: the cloud serves the portal and the API together. */
	baseUrl?: string;
	fetch?: typeof fetch;
}

interface ErrorBody {
	error?: {
		code?: string;
		message?: string;
		fields?: Record<string, string>;
		methods?: StepUpMethod[];
		retry_after?: number;
	};
}

const CODE_BY_STATUS: Record<number, ApiErrorCode> = {
	401: "unauthorized",
	403: "forbidden",
	404: "not_found",
	409: "conflict",
	400: "invalid",
	422: "invalid",
	429: "rate_limited",
};

export function createHttpApi(options: HttpApiOptions = {}): PortalApi {
	const base = `${options.baseUrl ?? ""}/web/api/v1`;
	const doFetch: typeof fetch = options.fetch ?? ((...a) => fetch(...a));
	let csrf: string | null = null;

	async function raw(method: string, path: string, body?: unknown, signal?: AbortSignal) {
		const headers: Record<string, string> = { Accept: "application/json" };
		if (body !== undefined) headers["Content-Type"] = "application/json";
		if (method !== "GET") {
			if (csrf === null) await session();
			if (csrf) headers["X-CSRF-Token"] = csrf;
		}
		let res: Response;
		try {
			res = await doFetch(`${base}${path}`, {
				method,
				headers,
				credentials: "same-origin",
				body: body === undefined ? undefined : JSON.stringify(body),
				signal,
			});
		} catch (e) {
			if (e instanceof DOMException && e.name === "AbortError") {
				throw new ApiError("passkey_cancelled", "Request cancelled.");
			}
			throw new ApiError("network", "Can't reach AnyPlay.");
		}
		if (res.ok) return res;

		let parsed: ErrorBody = {};
		try {
			parsed = (await res.json()) as ErrorBody;
		} catch {
			// not JSON; the status is all we have
		}
		const e = parsed.error ?? {};
		let code: ApiErrorCode =
			res.status >= 500 ? "server" : (CODE_BY_STATUS[res.status] ?? "unknown");
		if (e.code === "step_up_required") code = "step_up_required";
		throw new ApiError(code, e.message ?? "The request failed.", {
			status: res.status,
			fields: e.fields,
			methods: e.methods,
			retryAfter: e.retry_after ?? null,
		});
	}

	async function json<T>(
		method: string,
		path: string,
		body?: unknown,
		signal?: AbortSignal,
	): Promise<T> {
		const res = await raw(method, path, body, signal);
		if (res.status === 204) return undefined as T;
		return camelize<T>(await res.json());
	}

	async function session(): Promise<SessionInfo> {
		let res: Response;
		try {
			res = await doFetch(`${base}/session`, {
				headers: { Accept: "application/json" },
				credentials: "same-origin",
			});
		} catch {
			throw new ApiError("network", "Can't reach AnyPlay.");
		}
		if (!res.ok)
			throw new ApiError(res.status >= 500 ? "server" : "unknown", "Session check failed.", {
				status: res.status,
			});
		const info = camelize<SessionInfo>(await res.json());
		csrf = info.csrf;
		return info;
	}

	const passkeyRound = async (
		optionsPath: string,
		verifyPath: string,
		{ conditional, signal }: { conditional: boolean; signal?: AbortSignal },
	) => {
		const { options, id } = await json<{ options: RequestOptionsJSON; id: string }>(
			"POST",
			optionsPath,
		);
		const credential: CredentialJSON = await getAssertion(options, { conditional, signal });
		return json<AuthProgress>("POST", verifyPath, { id, credential }, signal);
	};

	return {
		mode: "http",
		session,

		auth: {
			start: (input) => json<CodeChallenge>("POST", "/auth/email", input),
			resendCode: () => json<CodeChallenge>("POST", "/auth/code/resend"),
			verifyCode: (code) => json<AuthProgress>("POST", "/auth/code", { code }),
			submitDetails: (input) => json<AuthProgress>("POST", "/auth/details", snakeize(input)),
			acceptTerms: () => json<AuthProgress>("POST", "/auth/terms", { accept: true }),
			passkeySupport,
			signInWithPasskey: (opts) =>
				passkeyRound("/auth/passkey/options", "/auth/passkey/verify", opts),
			signOut: async () => {
				await json<void>("POST", "/auth/sign-out");
				csrf = null;
			},
		},

		account: {
			updateProfile: (input) => json<Me>("PUT", "/account/profile", snakeize(input)),
			activity: () => json<ActivityEvent[]>("GET", "/account/activity"),
		},

		security: {
			passkeys: () => json<Passkey[]>("GET", "/account/passkeys"),
			addPasskey: async (name) => {
				const { options, id } = await json<{ options: Record<string, unknown>; id: string }>(
					"POST",
					"/account/passkeys/options",
				);
				const credential = await createCredential(options);
				return json<Passkey>("POST", "/account/passkeys", { id, name, credential });
			},
			removePasskey: (id) => json<void>("DELETE", `/account/passkeys/${encodeURIComponent(id)}`),
			sessions: () => json<DeviceSession[]>("GET", "/account/sessions"),
			revokeSession: (id) => json<void>("DELETE", `/account/sessions/${encodeURIComponent(id)}`),
			signOutEverywhere: () => json<void>("POST", "/account/sessions/sign-out-everywhere"),
		},

		stepUp: {
			methods: async () =>
				(await json<{ methods: StepUpMethod[] }>("GET", "/account/step-up")).methods,
			withPasskey: async () => {
				await passkeyRound("/account/step-up/passkey/options", "/account/step-up/passkey/verify", {
					conditional: false,
				});
			},
			sendEmailCode: () => json<CodeChallenge>("POST", "/account/step-up/email"),
			verifyEmailCode: (code) => json<void>("POST", "/account/step-up/email/verify", { code }),
		},

		privacy: {
			get: () => json<PrivacySettings>("GET", "/account/privacy"),
			update: (input) => json<PrivacySettings>("PUT", "/account/privacy", snakeize(input)),
		},

		data: {
			exportData: async () => {
				const res = await raw("GET", "/account/export");
				const blob = await res.blob();
				const match = /filename="?([^";]+)"?/.exec(res.headers.get("content-disposition") ?? "");
				return { filename: match?.[1] ?? "anyplay-data.json", blob };
			},
			requestDeletion: (input) => json<Me>("POST", "/account/delete", input),
			cancelDeletion: () => json<Me>("POST", "/account/delete/cancel"),
		},

		friends: {
			overview: () => json<FriendsOverview>("GET", "/friends"),
			sendRequest: (handle) => json<void>("POST", "/friends/requests", { handle }),
			accept: (id) => json<void>("POST", `/friends/requests/${encodeURIComponent(id)}/accept`),
			decline: (id) => json<void>("POST", `/friends/requests/${encodeURIComponent(id)}/decline`),
			cancel: (id) => json<void>("DELETE", `/friends/requests/${encodeURIComponent(id)}`),
			remove: (id) => json<void>("DELETE", `/friends/${encodeURIComponent(id)}`),
		},

		trophies: {
			summary: () => json<TrophySummary>("GET", "/trophies"),
		},

		profiles: {
			get: (handle) => json<PublicProfile>("GET", `/profiles/${encodeURIComponent(handle)}`),
		},
	};
}
