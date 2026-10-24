import { camelize, snakeize } from "./case";
import { ApiError, asErrorCode } from "./errors";
import type { PortalApi } from "./portal-api";
import type {
	AccountView,
	BanStatement,
	FriendLists,
	Passkey,
	Privacy,
	Profile,
	PublicUser,
	SessionView,
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
	error?: string;
	message?: string;
	field?: string;
}

const JSON_TYPE = "application/json";

export function createHttpApi(options: HttpApiOptions = {}): PortalApi {
	const base = `${options.baseUrl ?? ""}/web/api/v1`;
	const doFetch: typeof fetch = options.fetch ?? ((...a) => fetch(...a));
	let csrf: string | null = null;
	let passkeyCount = 0;

	async function raw(
		method: string,
		path: string,
		body?: unknown,
		extra: { signal?: AbortSignal; contentType?: string; rawBody?: BodyInit } = {},
	) {
		const headers: Record<string, string> = { Accept: JSON_TYPE };
		const write = method !== "GET";
		if (write) {
			// every write is JSON, even an empty one: the cloud refuses anything else
			headers["Content-Type"] = extra.contentType ?? JSON_TYPE;
			if (csrf === null) await session();
			if (csrf) headers["X-CSRF-Token"] = csrf;
		}
		let res: Response;
		try {
			res = await doFetch(`${base}${path}`, {
				method,
				headers,
				credentials: "same-origin",
				body: extra.rawBody ?? (write ? JSON.stringify(body ?? {}) : undefined),
				signal: extra.signal,
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
			// not JSON (an origin-guard refusal is HTML); the status is all we have
		}
		const code = asErrorCode(parsed.error, res.status);
		const retry = Number(res.headers.get("retry-after"));
		throw new ApiError(code, parsed.message ?? "The request failed.", {
			status: res.status,
			fields: parsed.field && parsed.message ? { [parsed.field]: parsed.message } : {},
			methods: code === "step_up_required" ? stepUpMethods() : [],
			retryAfter: Number.isFinite(retry) && retry > 0 ? retry : null,
		});
	}

	const stepUpMethods = (): StepUpMethod[] =>
		passkeyCount > 0 ? ["passkey", "email_code"] : ["email_code"];

	async function json<T>(
		method: string,
		path: string,
		body?: unknown,
		extra?: { signal?: AbortSignal },
	): Promise<T> {
		const res = await raw(method, path, body, extra);
		if (res.status === 204) return undefined as T;
		return camelize<T>(await res.json());
	}

	async function session(): Promise<SessionView> {
		let res: Response;
		try {
			res = await doFetch(`${base}/session`, {
				headers: { Accept: JSON_TYPE },
				credentials: "same-origin",
			});
		} catch {
			throw new ApiError("network", "Can't reach AnyPlay.");
		}
		if (!res.ok)
			throw new ApiError(asErrorCode(undefined, res.status), "Session check failed.", {
				status: res.status,
			});
		let view: SessionView;
		try {
			view = camelize<SessionView>(await res.json());
		} catch {
			// something answered, but not the API (static files with no cloud behind them)
			throw new ApiError("network", "That is not the AnyPlay API.", { status: res.status });
		}
		csrf = view.csrf;
		return view;
	}

	/** A step answers with the session view; a handoff or sign-in changes the session's token. */
	const step = async (path: string, body?: unknown): Promise<SessionView> => {
		const view = await json<SessionView>("POST", path, body);
		csrf = view.csrf;
		return view;
	};

	const passkeyRound = async (
		verifyPath: string,
		{ conditional, signal }: { conditional: boolean; signal?: AbortSignal },
	) => {
		const { options, id } = await json<{ options: RequestOptionsJSON; id: string }>(
			"POST",
			"/passkey/sign-in/start",
			{},
			{ signal },
		);
		const credential: CredentialJSON = await getAssertion(options, { conditional, signal });
		return { verifyPath, body: { id, credential } };
	};

	return {
		mode: "http",
		session,

		auth: {
			start: (input) => step("/sign-in/email", input),
			resendCode: () => step("/sign-in/code/resend"),
			verifyCode: (code) => step("/sign-in/code", { code }),
			submitDetails: (input) => step("/sign-up/details", snakeize(input)),
			acceptTerms: () => step("/terms", { accept: true }),
			passkeySupport,
			signInWithPasskey: async (opts) => {
				const { verifyPath, body } = await passkeyRound("/passkey/sign-in/finish", opts);
				return step(verifyPath, body);
			},
			signOut: async () => {
				await json("POST", "/sign-out");
				csrf = null;
			},
			ban: () => json<BanStatement>("GET", "/ban"),
			appeal: async (text) => {
				await json("POST", "/ban/appeal", { text });
			},
		},

		account: {
			get: async () => {
				const view = await json<AccountView>("GET", "/account");
				passkeyCount = view.passkeys.length;
				return view;
			},
			changeEmail: async (email) => {
				await json("POST", "/account/email", { email });
			},
			confirmEmail: async (code) =>
				(await json<{ email: string }>("POST", "/account/email/confirm", { code })).email,
			profile: () => json<Profile>("GET", "/me/profile"),
			updateProfile: (input) => json<Profile>("PATCH", "/me/profile", input),
			setAvatar: async (file) => {
				const type = file.type === "image/png" ? "image/png" : "image/jpeg";
				const res = await raw("PUT", "/me/avatar", undefined, { contentType: type, rawBody: file });
				return camelize<{ avatar: string }>(await res.json()).avatar;
			},
			removeAvatar: async () => {
				await json("DELETE", "/me/avatar");
			},
			privacy: () => json<Privacy>("GET", "/me/privacy"),
			updatePrivacy: (input) => json<Privacy>("PUT", "/me/privacy", input),
		},

		security: {
			addPasskey: async (name) => {
				const { options, id } = await json<{ options: Record<string, unknown>; id: string }>(
					"POST",
					"/account/passkeys/start",
				);
				const credential = await createCredential(options);
				const { passkey } = await json<{ passkey: Passkey }>("POST", "/account/passkeys/finish", {
					id,
					name,
					credential,
				});
				return passkey;
			},
			renamePasskey: async (id, name) => {
				await json("PATCH", `/account/passkeys/${encodeURIComponent(id)}`, { name });
			},
			removePasskey: async (id) => {
				await json("DELETE", `/account/passkeys/${encodeURIComponent(id)}`);
			},
			removeDevice: async (id) => {
				await json("DELETE", `/account/devices/${id}`);
			},
			revokeSession: (id) => json<{ signedOut: boolean }>("DELETE", `/account/sessions/${id}`),
			signOutEverywhere: async () => {
				await json("POST", "/account/sign-out-everywhere");
				csrf = null;
			},
		},

		stepUp: {
			methods: async () => stepUpMethods(),
			withPasskey: async () => {
				const { verifyPath, body } = await passkeyRound("/step-up/passkey/finish", {
					conditional: false,
				});
				await json("POST", verifyPath, body);
			},
			sendEmailCode: async () =>
				(await json<{ emailHint: string }>("POST", "/step-up/email")).emailHint,
			verifyEmailCode: async (code) => {
				await json("POST", "/step-up/code", { code });
			},
		},

		data: {
			exportData: async () => {
				const res = await raw("GET", "/account/export");
				const blob = await res.blob();
				const match = /filename="?([^";]+)"?/.exec(res.headers.get("content-disposition") ?? "");
				return { filename: match?.[1] ?? "anyplay-data.json", blob };
			},
			requestDeletion: async (handle) =>
				(await json<{ deletion: { at: number } }>("POST", "/account/deletion", { handle }))
					.deletion,
			cancelDeletion: async () => {
				await json("DELETE", "/account/deletion");
			},
		},

		friends: {
			lists: () => json<FriendLists>("GET", "/friends"),
			sendRequest: async (handle) =>
				(await json<{ state: "requested" | "friends" }>("POST", "/friends/requests", { handle }))
					.state,
			accept: async (id) => {
				await json("POST", `/friends/requests/${id}/accept`);
			},
			dropRequest: async (id) => {
				await json("DELETE", `/friends/requests/${id}`);
			},
			remove: async (id) => {
				await json("DELETE", `/friends/${id}`);
			},
			block: async (id) => {
				await json("PUT", `/blocks/${id}`);
			},
			unblock: async (id) => {
				await json("DELETE", `/blocks/${id}`);
			},
		},

		trophies: {
			mine: () => json<TrophySummary>("GET", "/me/trophies"),
		},

		users: {
			get: (name) => json<PublicUser>("GET", `/users/${encodeURIComponent(name)}`),
		},
	};
}
