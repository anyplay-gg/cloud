import { describe, expect, it, vi } from "vitest";
import { ApiError } from "./errors";
import { createHttpApi } from "./http-adapter";

const json = (body: unknown, init: ResponseInit = {}) =>
	new Response(JSON.stringify(body), {
		headers: { "content-type": "application/json" },
		...init,
	});

const view = (over: Record<string, unknown> = {}) => ({
	csrf: "tok-1",
	signed_in: true,
	account: { id: 41, handle: "riley", email: "riley@example.com", created: 1_700_000_000 },
	fresh_until: 1_800_000_000,
	flow: { step: "done" },
	authorize: null,
	notice: null,
	cloud: {
		name: "AnyPlay",
		signup: "open",
		privacy_url: null,
		terms_url: "https://x.example/terms",
	},
	...over,
});

function setup(handler: (url: string, init: RequestInit) => Response | Promise<Response>) {
	const fetchMock = vi.fn(async (input: RequestInfo | URL, init: RequestInit = {}) =>
		handler(String(input), init),
	);
	const api = createHttpApi({ fetch: fetchMock as unknown as typeof fetch });
	const call = (i: number) => ({
		url: String(fetchMock.mock.calls[i]?.[0]),
		init: (fetchMock.mock.calls[i]?.[1] ?? {}) as RequestInit,
	});
	return { api, fetchMock, call };
}

const headersOf = (init: RequestInit) => init.headers as Record<string, string>;

describe("http adapter", () => {
	it("calls the same origin with the cookie and reads snake_case as camelCase", async () => {
		const { api, call } = setup(() => json(view()));
		const s = await api.session();
		expect(s.signedIn).toBe(true);
		expect(s.csrf).toBe("tok-1");
		expect(s.freshUntil).toBe(1_800_000_000);
		expect(s.cloud.termsUrl).toBe("https://x.example/terms");
		expect(call(0).url).toBe("/web/api/v1/session");
		expect(call(0).init.credentials).toBe("same-origin");
	});

	it("sends JSON and X-CSRF-Token on every write, even an empty one", async () => {
		const { api, call } = setup((url) =>
			url.endsWith("/session") ? json(view({ csrf: "tok-9" })) : json({ ok: true }),
		);
		await api.security.removePasskey("pk 1");
		expect(call(0).url).toBe("/web/api/v1/session"); // no token yet: it asks first
		expect(call(1).url).toBe("/web/api/v1/account/passkeys/pk%201");
		expect(call(1).init.method).toBe("DELETE");
		expect(headersOf(call(1).init)["X-CSRF-Token"]).toBe("tok-9");
		expect(headersOf(call(1).init)["Content-Type"]).toBe("application/json");
		expect(call(1).init.body).toBe("{}");
	});

	it("takes the new token a sign-in step hands back", async () => {
		const { api, call } = setup((url) => {
			if (url.endsWith("/session"))
				return json(view({ csrf: "old", signed_in: false, flow: { step: "email" } }));
			if (url.endsWith("/sign-in/code")) return json(view({ csrf: "new" }));
			return json({ ok: true });
		});
		await api.session();
		await api.auth.verifyCode("123456");
		await api.security.removeDevice(3);
		expect(headersOf(call(2).init)["X-CSRF-Token"]).toBe("new");
	});

	it("does not send a CSRF token or a body on reads", async () => {
		const { api, call } = setup(() => json({ passkeys: [], devices: [], sessions: [] }));
		await api.account.get();
		expect(headersOf(call(0).init)["X-CSRF-Token"]).toBeUndefined();
		expect(call(0).init.body).toBeUndefined();
	});

	it("sends the avatar as the picture with its own content type", async () => {
		const { api, call } = setup((url) =>
			url.endsWith("/session") ? json(view()) : json({ avatar: "abc" }),
		);
		const hash = await api.account.setAvatar(new Blob(["x"], { type: "image/png" }));
		expect(hash).toBe("abc");
		expect(headersOf(call(1).init)["Content-Type"]).toBe("image/png");
		expect(call(1).init.method).toBe("PUT");
	});

	it("turns 403 step_up_required into a typed error, with the methods the account can use", async () => {
		const { api } = setup((url) => {
			if (url.endsWith("/session")) return json(view());
			if (url.endsWith("/account"))
				return json({
					account: view().account,
					passkeys: [{ id: "a", name: "A", created: 1, last_used: null }],
					max_passkeys: 10,
					devices: [],
					sessions: [],
					deletion: null,
					pending_email: null,
					fresh_until: 1,
				});
			return json(
				{ error: "step_up_required", message: "Confirm that it is you to do this." },
				{ status: 403 },
			);
		});
		await api.account.get(); // learns that the account has a passkey
		const err = await api.security.removePasskey("pk").catch((e) => e);
		expect(err).toBeInstanceOf(ApiError);
		expect(err.code).toBe("step_up_required");
		expect(err.methods).toEqual(["passkey", "email_code"]);
	});

	it("offers only the e-mail code to an account with no passkey", async () => {
		const { api } = setup((url) =>
			url.endsWith("/session")
				? json(view())
				: json({ error: "step_up_required", message: "x" }, { status: 403 }),
		);
		const err = await api.security.removePasskey("pk").catch((e) => e);
		expect(err.methods).toEqual(["email_code"]);
	});

	it("keeps a plain 403 as forbidden and a banned one as banned", async () => {
		const a = setup(() => json({ error: "forbidden", message: "no" }, { status: 403 }));
		expect((await a.api.account.get().catch((e) => e)).code).toBe("forbidden");
		const b = setup(() => json({ error: "banned", message: "no" }, { status: 403 }));
		expect((await b.api.account.get().catch((e) => e)).code).toBe("banned");
	});

	it.each([
		[400, "bad_request", "bad_request"],
		[400, "code_wrong", "code_wrong"],
		[400, "code_expired", "code_expired"],
		[401, "unauthorized", "unauthorized"],
		[404, "not_found", "not_found"],
		[409, "handle_taken", "handle_taken"],
		[409, "limit", "limit"],
		[413, "too_large", "too_large"],
		[415, "unsupported_media_type", "unsupported_media_type"],
		[429, "rate_limited", "rate_limited"],
		[500, "internal", "internal"],
		[502, undefined, "internal"],
	])("maps %i %s to %s", async (status, code, expected) => {
		const { api } = setup(() =>
			json({ error: code, message: "x" }, { status, headers: { "retry-after": "7" } }),
		);
		const err = await api.account.get().catch((e) => e);
		expect(err.code).toBe(expected);
		if (expected === "rate_limited") expect(err.retryAfter).toBe(7);
	});

	it("puts the input a problem names under that input", async () => {
		const { api } = setup((url) =>
			url.endsWith("/session")
				? json(view({ signed_in: false, flow: { step: "details" } }))
				: json(
						{ error: "handle_taken", message: "That handle is taken.", field: "handle" },
						{ status: 409 },
					),
		);
		const err = await api.auth
			.submitDetails({ handle: "x", ageConfirmed: true, termsAccepted: true })
			.catch((e) => e);
		expect(err.code).toBe("handle_taken");
		expect(err.fields.handle).toBe("That handle is taken.");
	});

	it("reports an unreachable cloud as a network error", async () => {
		const api = createHttpApi({
			fetch: (() => Promise.reject(new TypeError("Failed to fetch"))) as unknown as typeof fetch,
		});
		expect((await api.session().catch((e) => e)).code).toBe("network");
	});

	it("treats an HTML answer where JSON was expected as no API (static files with no cloud)", async () => {
		const api = createHttpApi({
			fetch: (async () =>
				new Response("<!doctype html>", {
					headers: { "content-type": "text/html" },
				})) as unknown as typeof fetch,
		});
		await expect(api.session()).rejects.toMatchObject({ code: "network" });
	});

	it("snake_cases what it builds and leaves the passkey credential alone", async () => {
		const { api, call } = setup((url) => (url.endsWith("/session") ? json(view()) : json(view())));
		await api.auth.submitDetails({ handle: "riley", ageConfirmed: true, termsAccepted: true });
		expect(JSON.parse(String(call(1).init.body))).toEqual({
			handle: "riley",
			age_confirmed: true,
			terms_accepted: true,
		});
	});

	it("starts a sign-in with the intent and reads the flow back", async () => {
		const { api, call } = setup((url) =>
			url.endsWith("/session")
				? json(view({ signed_in: false, flow: { step: "email" } }))
				: json(view({ signed_in: false, flow: { step: "code", email: "a@b.c", create: true } })),
		);
		const v = await api.auth.start({ email: "a@b.c", intent: "create" });
		expect(v.flow).toEqual({ step: "code", email: "a@b.c", create: true });
		expect(call(1).url).toBe("/web/api/v1/sign-in/email");
		expect(JSON.parse(String(call(1).init.body))).toEqual({ email: "a@b.c", intent: "create" });
	});
});
