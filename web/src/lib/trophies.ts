import type { GradeCounts } from "@/api/types";

export const GRADES = ["platinum", "gold", "silver", "bronze"] as const;
export type Grade = (typeof GRADES)[number];

export const GRADE_LABEL: Record<Grade, string> = {
	platinum: "Platinum",
	gold: "Gold",
	silver: "Silver",
	bronze: "Bronze",
};

/** How many trophies in all, whatever their grade. */
export const total = (c: GradeCounts) => GRADES.reduce((n, g) => n + c[g], 0);
