import { emptyCounts, percentOf } from "@/lib/trophies";
import { ApiError } from "../errors";
import type { PortalApi } from "../portal-api";
import type {
	ActivityEvent,
	ActivityKind,
	Audience,
	CodeChallenge,
	DeviceSession,
	Friend,
	FriendRequest,
	FriendsOverview,
	Me,
	Passkey,
	PrivacySettings,
	PublicProfile,
	SessionInfo,
	StepUpMethod,
	TrophySummary,
} from "../types";
import {
	defaultPrivacy,
	iso,
	type MockPlayer,
	players,
	seedActivity,
	seedFriends,
	seedIncoming,
	seedMe,
	seedOutgoing,
	seedPasskeys,
	seedSessions,
	seedTrophies,
	toPerson,
} from "./seed";

/** What the mock pretends is going on. `?mock=empty` and friends pick one in the browser. */
export type MockScenario = "default" | "empty" | "error" | "slow" | "signedout";

export interface MockOptions {
	scenario?: MockScenario;
	/** Milliseconds per call, or [min, max]. 0 for tests. */
	latency?: number | [number, number];
	/** Start with a recent sign-in, so the first sensitive write needs no step-up. */
	fresh?: boolean;
	now?: () => number;
	/** The address a sign-in code "arrives" at. */
	email?: string;
}

const FRESH_FOR = 10 * 60_000;
const TAKEN = new Set(["admin", "root", "anyplay", ...players.map((p) => p.handle)]);

export function parseScenario(search: string): MockScenario {
	const v = new URLSearchParams(search).get("mock");
	return v === "empty" || v === "error" || v === "slow" || v === "signedout" ? v : "default";
}

export function createMockApi(options: MockOptions = {}): PortalApi {
	const scenario = options.scenario ?? "default";
	const now = options.now ?? (() => Date.now());
	const latency = options.latency ?? [160, 340];
	const t0 = now();

	let signedIn = scenario !== "signedout";
	let me: Me = seedMe(t0);
	let freshUntil = options.fresh ? t0 + FRESH_FOR : 0;
	let passkeys: Passkey[] = scenario === "empty" ? [] : seedPasskeys(t0);
	let sessions: DeviceSession[] =
		scenario === "empty" ? seedSessions(t0).slice(0, 1) : seedSessions(t0);
	let activity: ActivityEvent[] =
		scenario === "empty" ? seedActivity(t0).slice(0, 1) : seedActivity(t0);
	let privacy: PrivacySettings = defaultPrivacy();
	let friends: Friend[] = scenario === "empty" ? [] : seedFriends(t0);
	let incoming: FriendRequest[] = scenario === "empty" ? [] : seedIncoming(t0);
	let outgoing: FriendRequest[] = scenario === "empty" ? [] : seedOutgoing(t0);
	const trophyData: TrophySummary =
		scenario === "empty"
			? { titles: [], earned: emptyCounts(), total: emptyCounts(), percent: 0 }
			: seedTrophies(t0);

	// the sign-in in progress
	let flow: { email: string; intent: "sign_in" | "create"; codeSentAt: number } | null = null;
	let pendingDetails: { handle: string; displayName: string } | null = null;
	let stepUpCodeSent = false;

	const wait = async (): Promise<void> => {
		if (scenario === "slow") await new Promise<void>(() => {});
		const ms = Array.isArray(latency)
			? latency[0] + Math.random() * (latency[1] - latency[0])
			: latency;
		if (ms > 0) await new Promise((r) => setTimeout(r, ms));
	};

	/** Wraps a handler: waits like a network, and makes reads fail in the "error" scenario. */
	const call = async <T>(fn: () => T, { read = false, auth = true } = {}): Promise<T> => {
		await wait();
		if (scenario === "error" && read)
			throw new ApiError("server", "The mock is set to fail.", { status: 500 });
		if (auth && !signedIn) throw new ApiError("unauthorized", "Sign in first.", { status: 401 });
		return fn();
	};

	const log = (kind: ActivityKind, detail: string | null = null) => {
		activity = [
			{ id: `ev_${activity.length + 1}_${now()}`, kind, at: iso(now(), 0), detail },
			...activity,
		];
	};

	const requireFresh = () => {
		if (now() > freshUntil) {
			const methods: StepUpMethod[] =
				passkeys.length > 0 ? ["passkey", "email_code"] : ["email_code"];
			throw new ApiError("step_up_required", "Confirm it's you to do this.", {
				status: 403,
				methods,
			});
		}
	};
	const markFresh = () => {
		freshUntil = now() + FRESH_FOR;
	};

	const codeChallenge = (email: string): CodeChallenge => ({
		emailHint: hideEmail(email),
		resendAfter: 30,
	});

	function requireOk(code: string) {
		if (!/^\d{6}$/.test(code) || code === "000000") {
			throw new ApiError("invalid", "That code is wrong or has expired.", {
				status: 422,
				fields: { code: "That code is wrong or has expired. Request a new one." },
			});
		}
	}

	function viewerCan(audience: Audience, relation: PublicProfile["relation"]) {
		if (audience === "everyone") return true;
		if (audience === "friends") return relation === "friend" || relation === "self";
		return relation === "self";
	}

	function profileOf(p: MockPlayer, relation: PublicProfile["relation"]): PublicProfile {
		const base = {
			handle: p.handle,
			displayName: p.displayName,
			accent: p.accent,
			theme: p.theme,
			joinedAt: iso(t0, p.joinedDaysAgo * 86_400_000),
			relation,
		};
		const request: PublicProfile["request"] = outgoing.some((r) => r.person.id === p.id)
			? "outgoing"
			: incoming.some((r) => r.person.id === p.id)
				? "incoming"
				: "none";
		if (p.locked && relation !== "friend") {
			return {
				...base,
				visible: false,
				about: null,
				friendCount: null,
				trophies: null,
				status: null,
				hidden: [],
				request,
			};
		}
		const hidden: PublicProfile["hidden"] = [];
		const show = (f: keyof MockPlayer["fields"]) => {
			const ok = viewerCan(p.fields[f], relation);
			if (!ok) hidden.push({ field: f, audience: p.fields[f] });
			return ok;
		};
		const total = p.trophies.total;
		return {
			...base,
			visible: true,
			about: show("about") ? p.about : null,
			friendCount: show("friends") ? p.friendCount : null,
			trophies: show("trophies")
				? {
						earned: p.trophies.earned,
						total,
						percent: percentOf(p.trophies.earned, total),
					}
				: null,
			status: show("activity") ? p.status : null,
			hidden,
			request,
		};
	}

	const api: PortalApi = {
		mode: "mock",

		async session() {
			await wait();
			const info: SessionInfo = {
				signedIn,
				terms: { url: "https://anyplaynet.com/terms", version: "2026-10" },
				csrf: "mock-csrf",
				account: signedIn ? me : null,
			};
			return info;
		},

		auth: {
			start: (input) =>
				call(
					() => {
						flow = { email: input.email, intent: input.intent, codeSentAt: now() };
						return codeChallenge(input.email);
					},
					{ auth: false },
				),
			resendCode: () =>
				call(
					() => {
						if (!flow) throw new ApiError("unauthorized", "Start again.", { status: 401 });
						flow.codeSentAt = now();
						return codeChallenge(flow.email);
					},
					{ auth: false },
				),
			verifyCode: (code) =>
				call(
					() => {
						if (!flow) throw new ApiError("unauthorized", "Start again.", { status: 401 });
						requireOk(code);
						if (flow.intent === "create") return { next: "details" as const };
						signedIn = true;
						freshUntil = now() + FRESH_FOR;
						log("sign_in", "Chrome on Windows");
						return { next: "done" as const };
					},
					{ auth: false },
				),
			submitDetails: (input) =>
				call(
					() => {
						const handle = input.handle.trim().toLowerCase();
						if (!/^[a-z][a-z0-9_-]{2,15}$/.test(handle)) {
							throw new ApiError("invalid", "Invalid handle.", {
								status: 422,
								fields: {
									handle:
										"Use 3 to 16 letters, digits, hyphens or underscores. Start with a letter.",
								},
							});
						}
						if (TAKEN.has(handle)) {
							throw new ApiError("invalid", "Handle taken.", {
								status: 422,
								fields: { handle: "That handle is taken. Try another." },
							});
						}
						pendingDetails = { handle, displayName: input.displayName.trim() || handle };
						return { next: "terms" as const };
					},
					{ auth: false },
				),
			acceptTerms: () =>
				call(
					() => {
						if (!flow || !pendingDetails)
							throw new ApiError("unauthorized", "Start again.", { status: 401 });
						me = {
							...seedMe(t0),
							id: "acc_new",
							handle: pendingDetails.handle,
							displayName: pendingDetails.displayName,
							email: flow.email,
							about: "",
							createdAt: iso(now(), 0),
						};
						passkeys = [];
						friends = [];
						incoming = [];
						outgoing = [];
						sessions = seedSessions(t0).slice(0, 1);
						signedIn = true;
						freshUntil = now() + FRESH_FOR;
						return { next: "done" as const };
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
							signedIn = true;
							freshUntil = now() + FRESH_FOR;
							log("sign_in", "Chrome on Windows (passkey)");
							return { next: "done" as const };
						},
						{ auth: false },
					).then(resolve, reject);
				}),
			signOut: () =>
				call(
					() => {
						signedIn = false;
						flow = null;
					},
					{ auth: false },
				),
		},

		account: {
			updateProfile: (input) =>
				call(() => {
					if (input.about.length > 160) {
						throw new ApiError("invalid", "Too long.", {
							status: 422,
							fields: { about: "Keep it to 160 characters." },
						});
					}
					me = { ...me, ...input };
					log("profile_updated");
					return me;
				}),
			activity: () => call(() => activity, { read: true }),
		},

		security: {
			passkeys: () => call(() => passkeys, { read: true }),
			addPasskey: (name) =>
				call(() => {
					requireFresh();
					if (passkeys.length >= 10) {
						throw new ApiError("conflict", "Limit reached.", { status: 409 });
					}
					const key: Passkey = {
						id: `pk_${passkeys.length + 1}_${now()}`,
						name: name.trim() || "Passkey",
						createdAt: iso(now(), 0),
						lastUsedAt: null,
						synced: false,
					};
					passkeys = [...passkeys, key];
					log("passkey_added", key.name);
					return key;
				}),
			removePasskey: (id) =>
				call(() => {
					requireFresh();
					const key = passkeys.find((k) => k.id === id);
					if (!key) throw new ApiError("not_found", "Not found.", { status: 404 });
					passkeys = passkeys.filter((k) => k.id !== id);
					log("passkey_removed", key.name);
				}),
			sessions: () => call(() => sessions, { read: true }),
			revokeSession: (id) =>
				call(() => {
					const s = sessions.find((x) => x.id === id);
					if (!s) throw new ApiError("not_found", "Not found.", { status: 404 });
					if (s.current)
						throw new ApiError("conflict", "Use sign out for this browser.", { status: 409 });
					sessions = sessions.filter((x) => x.id !== id);
					log("device_signed_out", s.name);
				}),
			signOutEverywhere: () =>
				call(() => {
					requireFresh();
					sessions = sessions.filter((x) => x.current);
					log("signed_out_everywhere");
				}),
		},

		stepUp: {
			methods: () =>
				call((): StepUpMethod[] =>
					passkeys.length > 0 ? ["passkey", "email_code"] : ["email_code"],
				),
			withPasskey: () => call(() => markFresh()),
			sendEmailCode: () =>
				call(() => {
					stepUpCodeSent = true;
					return codeChallenge(me.email);
				}),
			verifyEmailCode: (code) =>
				call(() => {
					if (!stepUpCodeSent)
						throw new ApiError("conflict", "Send a code first.", { status: 409 });
					requireOk(code);
					markFresh();
				}),
		},

		privacy: {
			get: () => call(() => privacy, { read: true }),
			update: (input) =>
				call(() => {
					privacy = input;
					return privacy;
				}),
		},

		data: {
			exportData: () =>
				call(() => {
					requireFresh();
					const body = JSON.stringify(
						{
							account: { handle: me.handle, email: me.email, created: me.createdAt },
							passkeys: passkeys.map((k) => ({ name: k.name, created: k.createdAt })),
							friends: friends.map((f) => f.handle),
							privacy,
							trophies: trophyData.titles.map((t) => ({ title: t.titleId, percent: t.percent })),
						},
						null,
						2,
					);
					return {
						filename: "anyplay-data.json",
						blob: new Blob([body], { type: "application/json" }),
					};
				}),
			requestDeletion: ({ handle }) =>
				call(() => {
					requireFresh();
					if (handle.trim().toLowerCase() !== me.handle) {
						throw new ApiError("invalid", "Wrong handle.", {
							status: 422,
							fields: { handle: "That is not your handle. Type it exactly to confirm." },
						});
					}
					me = {
						...me,
						deletion: {
							requestedAt: iso(now(), 0),
							scheduledFor: iso(now(), -30 * 86_400_000),
						},
					};
					return me;
				}),
			cancelDeletion: () =>
				call(() => {
					me = { ...me, deletion: null };
					return me;
				}),
		},

		friends: {
			overview: () =>
				call((): FriendsOverview => ({ friends, incoming, outgoing }), { read: true }),
			sendRequest: (handle) =>
				call(() => {
					const h = handle.trim().replace(/^@/, "").toLowerCase();
					const p = players.find((x) => x.handle === h);
					if (!p) {
						throw new ApiError("not_found", "No player with that handle.", {
							status: 404,
							fields: { handle: "No player has that handle. Check the spelling." },
						});
					}
					if (friends.some((f) => f.id === p.id)) {
						throw new ApiError("conflict", "Already friends.", {
							status: 409,
							fields: { handle: `You and ${p.displayName} are already friends.` },
						});
					}
					if (outgoing.some((r) => r.person.id === p.id)) {
						throw new ApiError("conflict", "Already sent.", {
							status: 409,
							fields: { handle: "You already sent this request." },
						});
					}
					const inc = incoming.find((r) => r.person.id === p.id);
					if (inc) {
						incoming = incoming.filter((r) => r.id !== inc.id);
						friends = [
							...friends,
							{ ...toPerson(p), status: p.status, activity: p.activity, since: iso(now(), 0) },
						];
						return;
					}
					outgoing = [
						{ id: `fr_out_${p.handle}`, person: toPerson(p), createdAt: iso(now(), 0) },
						...outgoing,
					];
				}),
			accept: (id) =>
				call(() => {
					const r = incoming.find((x) => x.id === id);
					if (!r) throw new ApiError("not_found", "Not found.", { status: 404 });
					const p = players.find((x) => x.id === r.person.id) as MockPlayer;
					incoming = incoming.filter((x) => x.id !== id);
					friends = [
						...friends,
						{ ...toPerson(p), status: p.status, activity: p.activity, since: iso(now(), 0) },
					];
				}),
			decline: (id) =>
				call(() => {
					incoming = incoming.filter((x) => x.id !== id);
				}),
			cancel: (id) =>
				call(() => {
					outgoing = outgoing.filter((x) => x.id !== id);
				}),
			remove: (id) =>
				call(() => {
					friends = friends.filter((x) => x.id !== id);
				}),
		},

		trophies: {
			summary: () => call(() => trophyData, { read: true }),
		},

		profiles: {
			get: async (handle) => {
				await wait();
				if (scenario === "error")
					throw new ApiError("server", "The mock is set to fail.", { status: 500 });
				const h = handle.toLowerCase();
				if (signedIn && h === me.handle) {
					return {
						handle: me.handle,
						displayName: me.displayName,
						accent: me.accent,
						theme: me.theme,
						joinedAt: me.createdAt,
						relation: "self",
						visible: true,
						about: me.about || null,
						friendCount: friends.length,
						trophies: {
							earned: trophyData.earned,
							total: trophyData.total,
							percent: trophyData.percent,
						},
						status: "online",
						hidden: [],
						request: "none",
					} satisfies PublicProfile;
				}
				const p = players.find((x) => x.handle === h);
				if (!p) throw new ApiError("not_found", "No such player.", { status: 404 });
				const relation: PublicProfile["relation"] = !signedIn
					? "signed_out"
					: friends.some((f) => f.id === p.id)
						? "friend"
						: "stranger";
				return profileOf(p, relation);
			},
		},
	};
	return api;
}

function hideEmail(email: string): string {
	const [user = "", domain = ""] = email.split("@");
	const head = user.slice(0, Math.min(2, user.length));
	return `${head}${"*".repeat(Math.max(3, user.length - head.length))}@${domain}`;
}
