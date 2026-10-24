// The screens both `pnpm shots` and `pnpm a11y` walk, with the mock scenario each one needs.
// `act` runs after load to open the state worth looking at (a menu, a dialog).

export const SCREENS = [
	{ id: "landing", path: "/?mock=signedout", ready: "Download the launcher" },
	{ id: "sign-in", path: "/sign-in?mock=signedout", ready: "Sign in to AnyPlay" },
	{
		id: "sign-in-code",
		path: "/sign-in?mock=signedout",
		ready: "Sign in to AnyPlay",
		act: async (p) => {
			await p.getByLabel("E-mail").fill("riley.chen@example.com");
			await p.getByRole("button", { name: "Send code" }).click();
			await p.getByRole("heading", { name: "Check your e-mail" }).waitFor();
			await p.keyboard.type("000000");
			await p.getByText(/wrong or has expired/).waitFor();
		},
	},
	{ id: "create", path: "/create?mock=signedout", ready: "Create your account" },
	{
		id: "create-details",
		path: "/create?mock=signedout",
		ready: "Create your account",
		act: async (p) => {
			await p.getByLabel("E-mail").fill("new@example.com");
			await p.getByRole("button", { name: "Send code" }).click();
			await p.getByRole("heading", { name: "Check your e-mail" }).waitFor();
			await p.keyboard.type("123456");
			await p.getByRole("heading", { name: "Choose a handle" }).waitFor();
			await p.getByLabel("Handle").fill("mira");
			await p.getByRole("checkbox", { name: /terms of service/ }).click();
			await p.getByRole("checkbox", { name: /old enough/ }).click();
			await p.getByRole("button", { name: "Create account" }).click();
			await p.getByText("That handle is taken. Try another.").waitFor();
		},
	},
	{ id: "return", path: "/return", ready: "You can return to AnyPlay" },
	{ id: "handoff", path: "/sign-in?mock=handoff", ready: "is waiting for you to sign in" },
	{ id: "banned", path: "/banned?mock=banned", ready: "This account is banned" },
	{ id: "overview", path: "/account", ready: "Signed in" },
	{ id: "overview-empty", path: "/account?mock=empty", ready: "Signed in" },
	{ id: "profile", path: "/account/profile", ready: "Preview" },
	{ id: "profile-empty", path: "/account/profile?mock=empty", ready: "Preview" },
	{ id: "friends", path: "/account/friends", ready: "Add a friend" },
	{
		id: "friends-requests",
		path: "/account/friends",
		ready: "Add a friend",
		act: async (p) => {
			await p.getByRole("tab", { name: /Requests/ }).click();
			await p.getByText("Waiting for your answer").waitFor();
		},
	},
	{ id: "friends-empty", path: "/account/friends?mock=empty", ready: "No friends yet" },
	{ id: "trophies", path: "/account/trophies", ready: "By game" },
	{ id: "trophies-empty", path: "/account/trophies?mock=empty", ready: "No trophies yet" },
	{
		id: "trophies-error",
		path: "/account/trophies?mock=error",
		ready: "Couldn't load your trophies",
	},
	{
		id: "trophies-loading",
		path: "/account/trophies?mock=slow",
		ready: "Your progress in each game",
		settle: 400,
	},
	{ id: "security", path: "/account/security", ready: "Other sessions" },
	{ id: "security-empty", path: "/account/security?mock=empty", ready: "No passkeys yet" },
	{
		id: "security-stepup",
		path: "/account/security",
		ready: "Other sessions",
		act: async (p) => {
			await p.getByRole("button", { name: "Remove passkey Phone" }).click();
			await p.getByRole("button", { name: "Confirm remove passkey Phone" }).click();
			await p.getByRole("dialog", { name: "Confirm it's you" }).waitFor();
		},
	},
	{ id: "privacy", path: "/account/privacy", ready: "Friend requests" },
	{ id: "data", path: "/account/data", ready: "Delete your account" },
	{
		id: "data-delete",
		path: "/account/data",
		ready: "Delete your account",
		act: async (p) => {
			await p.getByRole("button", { name: "Delete account" }).click();
			await p.getByRole("alertdialog").waitFor();
		},
	},
	{ id: "public-friend", path: "/u/mira", ready: "Favourite games" },
	{ id: "public-stranger", path: "/u/jonas", ready: "Racing, mostly" },
	{ id: "public-private", path: "/u/kenji", ready: "shares nothing else with you" },
	{ id: "public-signed-out", path: "/u/mira?mock=signedout", ready: "Favourite games" },
	{ id: "public-not-found", path: "/u/nobody-here", ready: "No player called" },
	{
		id: "menu-appearance",
		path: "/account",
		ready: "Signed in",
		act: async (p) => {
			await p.getByRole("button", { name: "Appearance" }).first().click();
			await p.getByRole("menu").waitFor();
		},
	},
];

export const VIEWPORTS = {
	desktop: { width: 1440, height: 900 },
	phone: { width: 390, height: 844 },
};
