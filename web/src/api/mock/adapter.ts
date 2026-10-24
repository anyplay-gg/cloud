import { ApiError } from "../errors";
import type { PortalApi } from "../portal-api";
import type {
	Account,
	AccountView,
	BanStatement,
	BrowserSession,
	Device,
	Flow,
	Friend,
	FriendLists,
	FriendRequest,
	GradeCounts,
	Me,
	Passkey,
	Privacy,
	Profile,
	PublicUser,
	SessionView,
	StepUpMethod,
	TrophySummary,
	Viewer,
} from "../types";
import {
	defaultPrivacy,
	ownPlaytime,
	players,
	presenceFor,
	seedDevices,
	seedFriends,
	seedIncoming,
	seedMe,
	seedOutgoing,
	seedPasskeys,
	seedSessions,
	seedTrophies,
	toAccount,
} from "./seed";

/** What the mock pretends is going on. `?mock=empty` and friends pick one in the browser. */
export type MockScenario =
	| "default"
	| "empty"
	| "error"
	| "slow"
	| "signedout"
	| "banned"
	| "handoff";

export interface MockOptions {
	scenario?: MockScenario;
	/** Milliseconds per call, or [min, max]. 0 for tests. */
	latency?: number | [number, number];
	/** Start with a recent sign-in, so the first sensitive write needs no step-up. */
	fresh?: boolean;
	now?: () => number;
}

const FRESH_FOR = 15 * 60;
const GRACE = 30 * 86_400;
const TAKEN = new Set(["admin", "root", "anyplay", ...players.map((p) => p.handle)]);
const SCENARIOS: MockScenario[] = ["empty", "error", "slow", "signedout", "banned", "handoff"];

export function parseScenario(search: string): MockScenario {
	const v = new URLSearchParams(search).get("mock") as MockScenario | null;
	return v && SCENARIOS.includes(v) ? v : "default";
}

const zero = (): GradeCounts => ({ platinum: 0, gold: 0, silver: 0, bronze: 0 });

export function createMockApi(options: MockOptions = {}): PortalApi {
	const scenario = options.scenario ?? "default";
	const nowMs = options.now ?? (() => Date.now());
	const now = () => Math.floor(nowMs() / 1000);
	const latency = options.latency ?? [160, 340];
	const t0 = now();

	const empty = scenario === "empty";
	let signedIn = scenario !== "signedout" && scenario !== "handoff";
	let me: Me = seedMe(t0);
	let freshUntil = options.fresh ? t0 + FRESH_FOR : 0;
	let passkeys: Passkey[] = empty ? [] : seedPasskeys(t0);
	let devices: Device[] = empty ? [] : seedDevices(t0);
	let sessions: BrowserSession[] = empty ? seedSessions(t0).slice(0, 1) : seedSessions(t0);
	let privacy: Privacy = defaultPrivacy();
	let profile = {
		about: empty ? "" : "Co-op when I can, solo when I can't. Always up for a rematch.",
		favourites: empty ? [] : ["GAME01234", "GAME04550"],
		avatar: null as string | null,
	};
	let friends: Friend[] = empty ? [] : seedFriends(t0);
	let incoming: FriendRequest[] = empty ? [] : seedIncoming(t0);
	let outgoing: FriendRequest[] = empty ? [] : seedOutgoing(t0);
	let blocked: Account[] = [];
	const trophyData: TrophySummary = empty ? { counts: zero(), titles: [] } : seedTrophies(t0);
	let deletion: { at: number } | null = null;
	let pendingEmail: string | null = null;
	let flow: Flow =
		scenario === "banned" ? { step: "banned" } : signedIn ? { step: "done" } : { step: "email" };
	const authorize = scenario === "handoff" ? { device: "Riley's desktop", create: false } : null;
	let notice: string | null = null;
	let pendingDetails: { handle: string } | null = null;
	let stepUpCodeSent = false;
	let appealFiled = false;

	const wait = async (keepGoing = false): Promise<void> => {
		// "slow" holds every read so the loading states can be seen; the session still answers
		if (scenario === "slow" && !keepGoing) await new Promise<void>(() => {});
		const ms = Array.isArray(latency)
			? latency[0] + Math.random() * (latency[1] - latency[0])
			: latency;
		if (ms > 0) await new Promise((r) => setTimeout(r, ms));
	};

	/** Wraps a handler: waits like a network, and makes reads fail in the "error" scenario. */
	const call = async <T>(fn: () => T, { read = false, auth = true } = {}): Promise<T> => {
		await wait();
		if (scenario === "error" && read) {
			throw new ApiError("internal", "The mock is set to fail.", { status: 500 });
		}
		if (auth && !signedIn) throw new ApiError("unauthorized", "Sign in first.", { status: 401 });
		return fn();
	};

	const methods = (): StepUpMethod[] =>
		passkeys.length > 0 ? ["passkey", "email_code"] : ["email_code"];
	const requireFresh = () => {
		if (now() > freshUntil) {
			throw new ApiError("step_up_required", "Confirm that it is you to do this.", {
				status: 403,
				methods: methods(),
			});
		}
	};
	const markFresh = () => {
		freshUntil = now() + FRESH_FOR;
	};

	const view = (): SessionView => ({
		csrf: "mock-csrf",
		signedIn,
		account: signedIn ? me : null,
		freshUntil: signedIn ? freshUntil : null,
		flow,
		authorize,
		notice,
		cloud: {
			name: "AnyPlay",
			signup: "open",
			privacyUrl: null,
			termsUrl: "https://anyplaynet.com/terms",
		},
	});

	const finishSignIn = () => {
		signedIn = true;
		markFresh();
		flow = authorize ? { step: "handoff", redirect: "/return" } : { step: "done" };
	};

	const badCode = (code: string) => {
		if (code === "000000") {
			throw new ApiError("code_wrong", "That code is wrong. Check it and try again.", {
				status: 400,
				fields: { code: "That code is wrong. Check it and try again." },
			});
		}
		if (!/^\d{6}$/.test(code)) {
			throw new ApiError("bad_request", "Enter the 6 digits.", { status: 400 });
		}
	};

	const hide = (email: string) => {
		const [user = "", domain = ""] = email.split("@");
		const head = user.slice(0, Math.min(2, user.length));
		return `${head}${"*".repeat(Math.max(3, user.length - head.length))}@${domain}`;
	};

	function can(audience: Privacy["about"], viewer: Viewer["relation"]) {
		if (audience === "everyone") return true;
		if (audience === "friends") return viewer === "friend" || viewer === "self";
		return viewer === "self";
	}

	const api: PortalApi = {
		mode: "mock",

		async session() {
			await wait(true);
			return view();
		},

		auth: {
			start: (input) =>
				call(
					() => {
						flow = { step: "code", email: input.email, create: input.intent === "create" };
						notice = null;
						return view();
					},
					{ auth: false },
				),
			resendCode: () =>
				call(
					() => {
						notice = "We sent a new code.";
						return view();
					},
					{ auth: false },
				),
			verifyCode: (code) =>
				call(
					() => {
						if (flow.step !== "code")
							throw new ApiError("unauthorized", "Start again.", { status: 401 });
						badCode(code);
						notice = null;
						if (flow.create) {
							flow = {
								step: "details",
								email: flow.email,
								terms: { version: "2026-10", url: "https://anyplaynet.com/terms" },
							};
							return view();
						}
						if (scenario === "banned") {
							flow = { step: "banned" };
							signedIn = true;
							return view();
						}
						finishSignIn();
						return view();
					},
					{ auth: false },
				),
			submitDetails: (input) =>
				call(
					() => {
						const handle = input.handle.trim().toLowerCase();
						if (!/^[a-z][a-z0-9_-]{2,15}$/.test(handle)) {
							throw new ApiError(
								"handle_invalid",
								"Use 3 to 16 letters, digits, hyphens or underscores. Start with a letter.",
								{
									status: 400,
									fields: {
										handle:
											"Use 3 to 16 letters, digits, hyphens or underscores. Start with a letter.",
									},
								},
							);
						}
						if (TAKEN.has(handle)) {
							throw new ApiError("handle_taken", "That handle is taken. Try another.", {
								status: 409,
								fields: { handle: "That handle is taken. Try another." },
							});
						}
						if (!input.ageConfirmed) {
							throw new ApiError("bad_request", "Confirm that you are old enough.", {
								status: 400,
								fields: { age_confirmed: "Confirm that you are old enough." },
							});
						}
						if (!input.termsAccepted) {
							throw new ApiError("bad_request", "Accept the terms to continue.", {
								status: 400,
								fields: { terms_accepted: "Accept the terms to continue." },
							});
						}
						pendingDetails = { handle };
						me = { id: 900, handle, email: flow.email ?? "new@example.com", created: now() };
						passkeys = [];
						friends = [];
						incoming = [];
						outgoing = [];
						devices = [];
						sessions = seedSessions(t0).slice(0, 1);
						profile = { about: "", favourites: [], avatar: null };
						finishSignIn();
						return view();
					},
					{ auth: false },
				),
			acceptTerms: () =>
				call(
					() => {
						finishSignIn();
						return view();
					},
					{ auth: false },
				),
			passkeySupport: async () => ({ available: true, conditional: true }),
			signInWithPasskey: ({ conditional, signal }) =>
				new Promise((resolve, reject) => {
					if (conditional) {
						// autofill waits for the person; the mock never answers by itself
						signal?.addEventListener("abort", () =>
							reject(new ApiError("passkey_cancelled", "Cancelled.")),
						);
						return;
					}
					call(
						() => {
							finishSignIn();
							return view();
						},
						{ auth: false },
					).then(resolve, reject);
				}),
			signOut: () =>
				call(
					() => {
						signedIn = false;
						flow = { step: "email" };
					},
					{ auth: false },
				),
			ban: () =>
				call(
					(): BanStatement => ({
						until: now() + 12 * 86_400,
						rule: "Harassment",
						reason: "Repeated abusive messages to other players in a party.",
						decided: now() - 2 * 86_400,
						appeal: appealFiled
							? { state: "filed", status: "open", answer: null }
							: { state: "open", deadline: now() + 12 * 86_400 },
					}),
					{ auth: false },
				),
			appeal: (text) =>
				call(
					() => {
						if (text.trim().length < 1 || text.length > 1000) {
							throw new ApiError("bad_request", "Write between 1 and 1,000 characters.", {
								status: 400,
								fields: { text: "Write between 1 and 1,000 characters." },
							});
						}
						appealFiled = true;
					},
					{ auth: false },
				),
		},

		account: {
			get: () =>
				call(
					(): AccountView => ({
						account: me,
						passkeys,
						maxPasskeys: 10,
						devices,
						sessions,
						deletion,
						pendingEmail,
						freshUntil: freshUntil || null,
					}),
					{ read: true },
				),
			changeEmail: (email) =>
				call(() => {
					requireFresh();
					if (!/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(email) || email === me.email) {
						throw new ApiError("bad_request", "Enter a different, valid address.", {
							status: 400,
							fields: { email: "Enter a different, valid address." },
						});
					}
					pendingEmail = email;
				}),
			confirmEmail: (code) =>
				call(() => {
					requireFresh();
					if (!pendingEmail)
						throw new ApiError("bad_request", "Ask for a code first.", { status: 400 });
					badCode(code);
					me = { ...me, email: pendingEmail };
					pendingEmail = null;
					return me.email;
				}),
			profile: () =>
				call(
					(): Profile => ({
						account: toAccount(me),
						about: profile.about,
						favourites: profile.favourites,
						avatar: profile.avatar,
						playtime: ownPlaytime,
					}),
					{ read: true },
				),
			updateProfile: (input) =>
				call((): Profile => {
					if (input.about !== undefined) {
						if (input.about.length > 160) {
							throw new ApiError("bad_request", "Keep it to 160 characters.", {
								status: 400,
								fields: { about: "Keep it to 160 characters." },
							});
						}
						profile = { ...profile, about: input.about };
					}
					if (input.favourites !== undefined)
						profile = { ...profile, favourites: input.favourites.slice(0, 3) };
					return { account: toAccount(me), ...profile, playtime: ownPlaytime };
				}),
			setAvatar: (file) =>
				call(() => {
					if (file.size > 1_000_000)
						throw new ApiError("too_large", "Pictures can be 1 MB at most.", { status: 413 });
					profile = { ...profile, avatar: "mock-avatar" };
					return "mock-avatar";
				}),
			removeAvatar: () =>
				call(() => {
					profile = { ...profile, avatar: null };
				}),
			privacy: () => call(() => privacy, { read: true }),
			updatePrivacy: (input) =>
				call(() => {
					privacy = input;
					return privacy;
				}),
		},

		security: {
			addPasskey: (name) =>
				call((): Passkey => {
					requireFresh();
					if (passkeys.length >= 10)
						throw new ApiError("limit", "You have 10 passkeys already.", { status: 409 });
					const key: Passkey = {
						id: `pk-${passkeys.length + 1}-${now()}`,
						name: name.trim() || `Passkey ${passkeys.length + 1}`,
						created: now(),
						lastUsed: null,
					};
					passkeys = [...passkeys, key];
					return key;
				}),
			renamePasskey: (id, name) =>
				call(() => {
					if (!passkeys.some((k) => k.id === id))
						throw new ApiError("not_found", "Not found.", { status: 404 });
					passkeys = passkeys.map((k) => (k.id === id ? { ...k, name: name.trim() || k.name } : k));
				}),
			removePasskey: (id) =>
				call(() => {
					requireFresh();
					if (!passkeys.some((k) => k.id === id))
						throw new ApiError("not_found", "Not found.", { status: 404 });
					passkeys = passkeys.filter((k) => k.id !== id);
				}),
			removeDevice: (id) =>
				call(() => {
					if (!devices.some((d) => d.id === id))
						throw new ApiError("not_found", "Not found.", { status: 404 });
					devices = devices.filter((d) => d.id !== id);
				}),
			revokeSession: (id) =>
				call(() => {
					const s = sessions.find((x) => x.id === id);
					if (!s) throw new ApiError("not_found", "Not found.", { status: 404 });
					sessions = sessions.filter((x) => x.id !== id);
					if (s.current) {
						signedIn = false;
						flow = { step: "email" };
					}
					return { signedOut: s.current };
				}),
			signOutEverywhere: () =>
				call(() => {
					requireFresh();
					devices = [];
					sessions = [];
					signedIn = false;
					flow = { step: "email" };
				}),
		},

		stepUp: {
			methods: () => call(methods),
			withPasskey: () => call(() => markFresh()),
			sendEmailCode: () =>
				call(() => {
					stepUpCodeSent = true;
					return hide(me.email);
				}),
			verifyEmailCode: (code) =>
				call(() => {
					if (!stepUpCodeSent)
						throw new ApiError("bad_request", "Ask for a code first.", { status: 400 });
					badCode(code);
					markFresh();
				}),
		},

		data: {
			exportData: () =>
				call(() => {
					requireFresh();
					const body = JSON.stringify(
						{
							account: { handle: me.handle, email: me.email, created: me.created },
							passkeys: passkeys.map((k) => ({ name: k.name, created: k.created })),
							friends: friends.map((f) => f.account.handle),
							privacy,
							trophies: trophyData.titles.map((t) => ({ title: t.titleId, earned: t.earned })),
						},
						null,
						2,
					);
					return {
						filename: "anyplay-data.json",
						blob: new Blob([body], { type: "application/json" }),
					};
				}),
			requestDeletion: (handle) =>
				call(() => {
					requireFresh();
					if (handle.trim().toLowerCase() !== me.handle) {
						throw new ApiError(
							"bad_request",
							"That is not your handle. Type it exactly to confirm.",
							{
								status: 400,
								fields: { handle: "That is not your handle. Type it exactly to confirm." },
							},
						);
					}
					deletion = deletion ?? { at: now() + GRACE };
					return deletion;
				}),
			cancelDeletion: () =>
				call(() => {
					deletion = null;
				}),
		},

		friends: {
			lists: () =>
				call((): FriendLists => ({ friends, incoming, outgoing, blocked }), { read: true }),
			sendRequest: (handle) =>
				call(() => {
					const h = handle.trim().replace(/^@/, "").toLowerCase();
					const p = players.find((x) => x.handle === h);
					if (!p) {
						throw new ApiError("not_found", "No player has that handle. Check the spelling.", {
							status: 404,
							fields: { handle: "No player has that handle. Check the spelling." },
						});
					}
					if (friends.some((f) => f.account.id === p.id)) {
						throw new ApiError("bad_request", `You and @${p.handle} are already friends.`, {
							status: 400,
							fields: { handle: `You and @${p.handle} are already friends.` },
						});
					}
					const inc = incoming.find((r) => r.account.id === p.id);
					if (inc) {
						incoming = incoming.filter((r) => r !== inc);
						friends = [
							...friends,
							{ account: toAccount(p), since: now(), presence: presenceFor(p) },
						];
						return "friends" as const;
					}
					if (!outgoing.some((r) => r.account.id === p.id)) {
						outgoing = [{ account: toAccount(p), since: now() }, ...outgoing];
					}
					return "requested" as const;
				}),
			accept: (id) =>
				call(() => {
					const r = incoming.find((x) => x.account.id === id);
					const p = players.find((x) => x.id === id);
					if (!r || !p) throw new ApiError("not_found", "Not found.", { status: 404 });
					incoming = incoming.filter((x) => x !== r);
					friends = [...friends, { account: toAccount(p), since: now(), presence: presenceFor(p) }];
				}),
			dropRequest: (id) =>
				call(() => {
					incoming = incoming.filter((x) => x.account.id !== id);
					outgoing = outgoing.filter((x) => x.account.id !== id);
				}),
			remove: (id) =>
				call(() => {
					friends = friends.filter((x) => x.account.id !== id);
				}),
			block: (id) =>
				call(() => {
					const p = players.find((x) => x.id === id);
					friends = friends.filter((x) => x.account.id !== id);
					incoming = incoming.filter((x) => x.account.id !== id);
					outgoing = outgoing.filter((x) => x.account.id !== id);
					if (p && !blocked.some((b) => b.id === id)) blocked = [...blocked, toAccount(p)];
				}),
			unblock: (id) =>
				call(() => {
					blocked = blocked.filter((b) => b.id !== id);
				}),
		},

		trophies: {
			mine: () => call(() => trophyData, { read: true }),
		},

		users: {
			get: async (handle) => {
				await wait();
				if (scenario === "error")
					throw new ApiError("internal", "The mock is set to fail.", { status: 500 });
				const h = handle.toLowerCase();
				if (signedIn && h === me.handle) {
					return {
						account: toAccount(me),
						about: profile.about || null,
						favourites: profile.favourites.length ? profile.favourites : null,
						avatar: profile.avatar,
						playtime: ownPlaytime,
						trophies: trophyData.counts,
						viewer: { relation: "self", requested: null, blocked: false },
					} satisfies PublicUser;
				}
				const p = players.find((x) => x.handle === h);
				if (!p) throw new ApiError("not_found", "No such player.", { status: 404 });
				const isBlocked = blocked.some((b) => b.id === p.id);
				const relation: Viewer["relation"] = friends.some((f) => f.account.id === p.id)
					? "friend"
					: "none";
				const viewer: Viewer | null = signedIn
					? {
							relation,
							requested: outgoing.some((r) => r.account.id === p.id)
								? "outgoing"
								: incoming.some((r) => r.account.id === p.id)
									? "incoming"
									: null,
							blocked: isBlocked,
						}
					: null;
				const seeAs = signedIn ? relation : "none";
				const part = <T>(k: keyof Privacy, v: T, empty: boolean): T | null =>
					isBlocked || !can(p.privacy[k], seeAs) || empty ? null : v;
				return {
					account: toAccount(p),
					about: part("about", p.about, !p.about),
					favourites: part("favourites", p.favourites, p.favourites.length === 0),
					avatar: part("avatar", p.avatar, !p.avatar),
					playtime: part("playtime", p.playtime, p.playtime.length === 0),
					trophies: part("trophies", p.trophies, false),
					viewer,
				};
			},
		},
	};
	void pendingDetails;
	return api;
}
