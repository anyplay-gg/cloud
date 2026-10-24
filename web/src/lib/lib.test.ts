import { describe, expect, it } from "vitest";
import { daysUntil, formatPlaytime, relativeTime } from "./format";
import { accentFor, themeFor } from "./identity";
import { authSearch } from "./search";
import { total } from "./trophies";

describe("trophy totals", () => {
	it("adds the grades", () => {
		expect(total({ platinum: 1, gold: 2, silver: 3, bronze: 4 })).toBe(10);
	});
});

describe("?next=", () => {
	it("accepts a path on this site and drops anything else", () => {
		expect(authSearch.parse({ next: "/account/security" }).next).toBe("/account/security");
		expect(authSearch.parse({ next: "https://evil.example" }).next).toBeUndefined();
		expect(authSearch.parse({ next: "//evil.example" }).next).toBeUndefined();
		expect(authSearch.parse({ next: "/\\evil" }).next).toBeUndefined();
	});
});

describe("time, in Unix seconds", () => {
	const now = Date.parse("2026-10-24T12:00:00Z") / 1000;
	it("says it in words", () => {
		expect(relativeTime(now - 10, now)).toMatch(/now|second/);
		expect(relativeTime(now - 2 * 3600, now)).toBe("2 hours ago");
		expect(relativeTime(now - 86_400, now)).toBe("yesterday");
	});
	it("counts whole days up to a time, never below zero", () => {
		expect(daysUntil(now + 30 * 86_400, now)).toBe(30);
		expect(daysUntil(now - 86_400, now)).toBe(0);
	});
	it("writes playtime", () => {
		expect(formatPlaytime(45)).toBe("45 min");
		expect(formatPlaytime(7260)).toBe("121 h");
	});
});

describe("identity", () => {
	it("gives the same player the same look every time", () => {
		expect(accentFor("riley")).toBe(accentFor("RILEY"));
		expect(themeFor("riley")).toBe(themeFor("riley"));
	});
});
