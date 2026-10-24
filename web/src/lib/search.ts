import { z } from "zod";

/** A path on this site and nothing else, so `?next=` cannot send anyone to another origin. */
export const safeNext = z
	.string()
	.refine((v) => v.startsWith("/") && !v.startsWith("//") && !v.includes("\\"))
	.optional()
	.catch(undefined);

export const authSearch = z.object({
	next: safeNext,
	handoff: z.boolean().optional().catch(undefined),
});
