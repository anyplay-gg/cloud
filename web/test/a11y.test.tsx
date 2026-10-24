import { screen, waitFor } from "@testing-library/react";
import axe from "axe-core";
import { describe, expect, it } from "vitest";
import type { MockScenario } from "@/api/mock/adapter";
import { renderRoute } from "./render";

// Every screen, signed in or out as it needs, through axe. jsdom has no layout or paint, so the
// colour-contrast rule is off here; `pnpm a11y` runs the full rule set, contrast included, in the
// installed Chrome against the built mock.
const screens: { name: string; path: string; ready: RegExp | string; scenario?: MockScenario }[] = [
	{ name: "landing", path: "/", ready: /Play together/, scenario: "signedout" },
	{ name: "sign in", path: "/sign-in", ready: "Sign in to AnyPlay", scenario: "signedout" },
	{ name: "create account", path: "/create", ready: "Create your account", scenario: "signedout" },
	{ name: "banned", path: "/sign-in", ready: "Rule", scenario: "banned" },
	{ name: "overview", path: "/account", ready: /Welcome back/ },
	{ name: "profile", path: "/account/profile", ready: "Preview" },
	{ name: "friends", path: "/friends", ready: "Add a friend" },
	{ name: "friends, empty", path: "/friends", ready: "No friends yet", scenario: "empty" },
	{ name: "trophies", path: "/trophies", ready: "By game" },
	{
		name: "trophies, empty",
		path: "/trophies",
		ready: "No trophies yet",
		scenario: "empty",
	},
	{
		name: "trophies, failed",
		path: "/trophies",
		ready: "Couldn't load your trophies",
		scenario: "error",
	},
	{ name: "security", path: "/account/security", ready: "Other sessions" },
	{ name: "privacy", path: "/account/privacy", ready: "Picture" },
	{ name: "data", path: "/account/data", ready: "Delete your account" },
	{ name: "public profile, friend", path: "/u/mira", ready: "Favourite games" },
	{ name: "public profile, stranger", path: "/u/jonas", ready: /Racing, mostly/ },
	{
		name: "public profile, signed out",
		path: "/u/mira",
		ready: "Favourite games",
		scenario: "signedout",
	},
	{ name: "not found", path: "/nothing-here", ready: "Page not found" },
];

describe("axe", () => {
	it.each(screens)("$name has no violations", async ({ path, ready, scenario }) => {
		const { container } = await renderRoute(path, { scenario });
		await waitFor(() => expect(screen.getAllByText(ready).length).toBeGreaterThan(0), {
			timeout: 4000,
		});
		const results = await axe.run(container.ownerDocument.body, {
			rules: { "color-contrast": { enabled: false }, region: { enabled: false } },
		});
		const summary = results.violations.map(
			(v) => `${v.id}: ${v.help}\n  ${v.nodes.map((n) => n.target.join(" ")).join("\n  ")}`,
		);
		expect(summary).toEqual([]);
	});
});
