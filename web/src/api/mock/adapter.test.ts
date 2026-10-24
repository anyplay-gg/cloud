import { describe, expect, it } from "vitest";
import { isStepUp } from "../errors";
import { createMockApi, parseScenario } from "./adapter";

const make = (o = {}) => createMockApi({ latency: 0, ...o });

describe("mock adapter", () => {
	it("starts signed in, and signed out for the signedout scenario", async () => {
		const a = await make().session();
		expect(a.signedIn).toBe(true);
		expect(a.flow.step).toBe("done");
		const b = await make({ scenario: "signedout" }).session();
		expect(b.signedIn).toBe(false);
		expect(b.flow.step).toBe("email");
	});

	it("asks for a step-up on a sensitive write until the person confirms", async () => {
		const api = make();
		const err = await api.security.removePasskey("pk-phone").catch((e) => e);
		expect(isStepUp(err)).toBe(true);
		expect(err.methods).toEqual(["passkey", "email_code"]);
		await api.stepUp.sendEmailCode();
		await api.stepUp.verifyEmailCode("123456");
		await api.security.removePasskey("pk-phone");
		expect((await api.account.get()).passkeys.map((k) => k.id)).not.toContain("pk-phone");
	});

	it("rejects a wrong code with the cloud's code_wrong", async () => {
		const api = make();
		await api.stepUp.sendEmailCode();
		const err = await api.stepUp.verifyEmailCode("000000").catch((e) => e);
		expect(err.code).toBe("code_wrong");
	});

	it("walks sign-in as the cloud's flow: email, code, details, done", async () => {
		const api = make({ scenario: "signedout" });
		expect((await api.auth.start({ email: "new@example.com", intent: "create" })).flow.step).toBe(
			"code",
		);
		const details = await api.auth.verifyCode("123456");
		expect(details.flow.step).toBe("details");
		expect(details.flow.terms?.version).toBeTruthy();
		const taken = await api.auth
			.submitDetails({ handle: "mira", ageConfirmed: true, termsAccepted: true })
			.catch((e) => e);
		expect(taken.code).toBe("handle_taken");
		expect(taken.fields.handle).toMatch(/taken/);
		const done = await api.auth.submitDetails({
			handle: "newbie",
			ageConfirmed: true,
			termsAccepted: true,
		});
		expect(done.flow.step).toBe("done");
		expect(done.account?.handle).toBe("newbie");
	});

	it("hands a launcher sign-in off with a redirect", async () => {
		const api = make({ scenario: "handoff" });
		expect((await api.session()).authorize).not.toBeNull();
		await api.auth.start({ email: "riley.chen@example.com", intent: "sign_in" });
		const v = await api.auth.verifyCode("123456");
		expect(v.flow.step).toBe("handoff");
		expect(v.flow.redirect).toBeTruthy();
	});

	it("shows a stranger less than a friend, and nothing a player keeps to themselves", async () => {
		const api = make();
		const mira = await api.users.get("mira");
		expect(mira.viewer?.relation).toBe("friend");
		expect(mira.trophies).not.toBeNull();
		const jonas = await api.users.get("jonas");
		expect(jonas.viewer?.relation).toBe("none");
		expect(jonas.trophies).toBeNull();
		expect(jonas.about).not.toBeNull();
		const kenji = await api.users.get("kenji");
		expect(kenji.about).toBeNull();
		expect(kenji.trophies).toBeNull();
		await expect(api.users.get("nobody-here")).rejects.toMatchObject({ code: "not_found" });
	});

	it("shows a visitor who is not signed in a null viewer", async () => {
		const u = await make({ scenario: "signedout" }).users.get("mira");
		expect(u.viewer).toBeNull();
	});

	it("never hands out trophy names, only title ids and counts", async () => {
		const t = await make().trophies.mine();
		for (const title of t.titles) {
			expect(Object.keys(title).sort()).toEqual(["counts", "earned", "lastEarned", "titleId"]);
		}
	});

	it("starts the 30-day grace period only for the right handle", async () => {
		const api = make({ fresh: true });
		await expect(api.data.requestDeletion("someone")).rejects.toMatchObject({
			code: "bad_request",
		});
		const { at } = await api.data.requestDeletion("Riley");
		expect(Math.round((at - Date.now() / 1000) / 86_400)).toBe(30);
		expect((await api.account.get()).deletion).not.toBeNull();
		await api.data.cancelDeletion();
		expect((await api.account.get()).deletion).toBeNull();
	});

	it("changes the e-mail through a code, behind the step-up", async () => {
		const api = make({ fresh: true });
		await api.account.changeEmail("new.address@example.com");
		expect((await api.account.get()).pendingEmail).toBe("new.address@example.com");
		expect(await api.account.confirmEmail("123456")).toBe("new.address@example.com");
		expect((await api.session()).account?.email).toBe("new.address@example.com");
	});

	it("fails reads in the error scenario", async () => {
		await expect(make({ scenario: "error" }).friends.lists()).rejects.toMatchObject({
			code: "internal",
		});
	});

	it("parses the scenario from a URL", () => {
		expect(parseScenario("?mock=empty")).toBe("empty");
		expect(parseScenario("?mock=banned")).toBe("banned");
		expect(parseScenario("?mock=whatever")).toBe("default");
		expect(parseScenario("")).toBe("default");
	});
});
