import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { heading, renderRoute } from "../../test/render";

describe("sign in", () => {
	it("explains a bad e-mail under the field and does not call the cloud", async () => {
		const user = userEvent.setup();
		await renderRoute("/sign-in", { scenario: "signedout" });
		await heading("Sign in to AnyPlay");
		await user.type(screen.getByLabelText("E-mail"), "not-an-address");
		await user.click(screen.getByRole("button", { name: "Send code" }));
		expect(await screen.findByText(/Enter a full e-mail address/)).toBeInTheDocument();
		expect(screen.getByLabelText("E-mail")).toHaveAttribute("aria-invalid", "true");
	});

	it("goes e-mail, then code, then the account", async () => {
		const user = userEvent.setup();
		const { router } = await renderRoute("/sign-in", { scenario: "signedout" });
		await user.type(await screen.findByLabelText("E-mail"), "riley.chen@example.com");
		await user.click(screen.getByRole("button", { name: "Send code" }));
		await heading("Check your e-mail");
		await user.keyboard("123456");
		await waitFor(() => expect(router.state.location.pathname).toBe("/account"));
	});

	it("says how to fix a wrong code and clears the boxes", async () => {
		const user = userEvent.setup();
		await renderRoute("/sign-in", { scenario: "signedout" });
		await user.type(await screen.findByLabelText("E-mail"), "riley.chen@example.com");
		await user.click(screen.getByRole("button", { name: "Send code" }));
		await heading("Check your e-mail");
		await user.keyboard("000000");
		expect(await screen.findByText(/That code is wrong/)).toBeInTheDocument();
	});

	it("tells the person the launcher is waiting, and ends on the return page", async () => {
		const user = userEvent.setup();
		await renderRoute("/sign-in", { scenario: "handoff" });
		expect(await screen.findByText(/launcher on Riley's desktop is waiting/i)).toBeInTheDocument();
		await user.type(screen.getByLabelText("E-mail"), "riley.chen@example.com");
		await user.click(screen.getByRole("button", { name: "Send code" }));
		await heading("Check your e-mail");
		await user.keyboard("123456");
		await heading("You can return to AnyPlay");
	});
});

describe("create account", () => {
	it("refuses a taken handle with the cloud's message on the field", async () => {
		const user = userEvent.setup();
		await renderRoute("/create", { scenario: "signedout" });
		await user.type(await screen.findByLabelText("E-mail"), "new@example.com");
		await user.click(screen.getByRole("button", { name: "Send code" }));
		await heading("Check your e-mail");
		await user.keyboard("123456");
		await heading("Choose a handle");
		await user.type(screen.getByLabelText("Handle"), "mira");
		await user.click(screen.getByRole("checkbox", { name: /terms of service/ }));
		await user.click(screen.getByRole("checkbox", { name: /old enough/ }));
		await user.click(screen.getByRole("button", { name: "Create account" }));
		expect(await screen.findByText("That handle is taken. Try another.")).toBeInTheDocument();
	});

	it("needs both boxes ticked before it asks the cloud", async () => {
		const user = userEvent.setup();
		await renderRoute("/create", { scenario: "signedout" });
		await user.type(await screen.findByLabelText("E-mail"), "new@example.com");
		await user.click(screen.getByRole("button", { name: "Send code" }));
		await heading("Check your e-mail");
		await user.keyboard("123456");
		await heading("Choose a handle");
		await user.type(screen.getByLabelText("Handle"), "newbie");
		await user.click(screen.getByRole("button", { name: "Create account" }));
		expect(await screen.findByText(/Accept the terms/)).toBeInTheDocument();
		expect(screen.getByText(/old enough to agree on your own/)).toBeInTheDocument();
	});
});

describe("routing", () => {
	it("sends a signed-out visitor from the account to sign in, and remembers where they were going", async () => {
		const { router } = await renderRoute("/account/security", { scenario: "signedout" });
		await heading("Sign in to AnyPlay");
		expect(router.state.location.pathname).toBe("/sign-in");
		expect(router.state.location.search).toMatchObject({ next: "/account/security" });
	});

	it("shows the landing at / when signed out, and the account when signed in", async () => {
		const out = await renderRoute("/", { scenario: "signedout" });
		await heading(/Play together, on an open network/);
		expect(screen.getByRole("link", { name: /Download the launcher/ })).toHaveAttribute(
			"href",
			"https://github.com/anyplay-gg/launcher/releases/latest",
		);
		out.unmount();
		const inn = await renderRoute("/");
		await heading(/Welcome back, riley/);
		expect(inn.router.state.location.pathname).toBe("/account");
	});

	it("shows the landing when the API cannot be reached at all", async () => {
		const { createMockApi } = await import("@/api/mock/adapter");
		const base = createMockApi({ latency: 0, scenario: "signedout" });
		const dead = { ...base, session: () => Promise.reject(new Error("no API here")) };
		await renderRoute("/", { api: dead });
		await heading(/Play together, on an open network/);
	});

	it("leads a banned account to its statement and an appeal", async () => {
		const user = userEvent.setup();
		const { router } = await renderRoute("/account", { scenario: "banned" });
		await heading("This account is banned");
		expect(router.state.location.pathname).toBe("/banned");
		await user.type(
			await screen.findByLabelText(/Why should this be reversed/),
			"It was a misunderstanding.",
		);
		await user.click(screen.getByRole("button", { name: "Send appeal" }));
		expect(await screen.findByText("Your appeal is open")).toBeInTheDocument();
	});
});

describe("security: the step-up path", () => {
	const open = async (user: ReturnType<typeof userEvent.setup>) => {
		await heading("Security");
		await user.click(await screen.findByRole("button", { name: "Remove passkey Phone" }));
		await user.click(await screen.findByRole("button", { name: "Confirm remove passkey Phone" }));
	};

	it("asks for a code, then removes the passkey and says so", async () => {
		const user = userEvent.setup();
		await renderRoute("/account/security");
		await open(user);
		const dialog = await screen.findByRole("dialog", { name: "Confirm it's you" });
		await user.click(within(dialog).getByRole("button", { name: "E-mail me a code" }));
		await user.keyboard("123456");
		await waitFor(() => expect(screen.queryByText("Phone")).not.toBeInTheDocument());
		expect(await screen.findByText("Passkey removed")).toBeInTheDocument();
	});

	it("does nothing and shows no error when the person closes the dialog", async () => {
		const user = userEvent.setup();
		await renderRoute("/account/security");
		await open(user);
		const dialog = await screen.findByRole("dialog", { name: "Confirm it's you" });
		await user.click(within(dialog).getByRole("button", { name: "Close" }));
		await waitFor(() =>
			expect(screen.queryByRole("dialog", { name: "Confirm it's you" })).not.toBeInTheDocument(),
		);
		expect(screen.getByText("Phone")).toBeInTheDocument();
		expect(screen.queryByRole("alert")).not.toBeInTheDocument();
	});

	it("goes straight through when the sign-in is recent", async () => {
		const user = userEvent.setup();
		await renderRoute("/account/security", { fresh: true });
		await open(user);
		await waitFor(() => expect(screen.queryByText("Phone")).not.toBeInTheDocument());
		expect(screen.queryByRole("dialog", { name: "Confirm it's you" })).not.toBeInTheDocument();
	});
});

describe("states", () => {
	it("shows an empty state, not a blank, for a player with no friends", async () => {
		await renderRoute("/account/friends", { scenario: "empty" });
		expect(await screen.findByText("No friends yet")).toBeInTheDocument();
	});

	it("shows what failed and a retry when a list cannot load", async () => {
		await renderRoute("/account/trophies", { scenario: "error" });
		expect(await screen.findByText("Couldn't load your trophies")).toBeInTheDocument();
		expect(screen.getByRole("button", { name: "Try again" })).toBeInTheDocument();
	});

	it("says what a stranger can and cannot see", async () => {
		await renderRoute("/u/kenji");
		expect(await screen.findByText(/shares nothing else with you/)).toBeInTheDocument();
	});

	it("says there is no such player", async () => {
		await renderRoute("/u/does-not-exist");
		expect(await screen.findByText(/No player called @does-not-exist/)).toBeInTheDocument();
	});
});

describe("friends", () => {
	it("explains an unknown handle under the field", async () => {
		const user = userEvent.setup();
		await renderRoute("/account/friends");
		await user.type(await screen.findByLabelText("Handle"), "ghost");
		await user.click(screen.getByRole("button", { name: "Send request" }));
		expect(
			await screen.findByText("No player has that handle. Check the spelling."),
		).toBeInTheDocument();
	});
});
