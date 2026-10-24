import type { GradeCounts } from "@/api/types";

export const GRADES = ["platinum", "gold", "silver", "bronze"] as const;
export type Grade = (typeof GRADES)[number];

/** Points per grade, the launcher's numbers (docs/DESIGN.md 1.2). */
export const POINTS: Record<Grade, number> = { platinum: 300, gold: 90, silver: 30, bronze: 15 };

export const GRADE_LABEL: Record<Grade, string> = {
	platinum: "Platinum",
	gold: "Gold",
	silver: "Silver",
	bronze: "Bronze",
};

export const sumPoints = (c: GradeCounts) => GRADES.reduce((n, g) => n + c[g] * POINTS[g], 0);

/** Whole percent of points unlocked, integer division, 0 when there is nothing to unlock. */
export function percentOf(earned: GradeCounts, total: GradeCounts): number {
	const t = sumPoints(total);
	return t === 0 ? 0 : Math.floor((sumPoints(earned) * 100) / t);
}

export const addCounts = (a: GradeCounts, b: GradeCounts): GradeCounts => ({
	platinum: a.platinum + b.platinum,
	gold: a.gold + b.gold,
	silver: a.silver + b.silver,
	bronze: a.bronze + b.bronze,
});

export const emptyCounts = (): GradeCounts => ({ platinum: 0, gold: 0, silver: 0, bronze: 0 });
