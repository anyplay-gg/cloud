import { type ClassValue, clsx } from "clsx";
import { extendTailwindMerge } from "tailwind-merge";

// The type scale has custom names (text-body, text-title, ...). Without this, tailwind-merge
// reads them as colours and drops a real text colour that sits next to them.
const twMerge = extendTailwindMerge({
	extend: {
		classGroups: {
			"font-size": [
				{
					text: [
						"caption",
						"small",
						"body",
						"prose",
						"lead",
						"title",
						"heading",
						"display",
						"hero",
					],
				},
			],
		},
	},
});

export const cn = (...inputs: ClassValue[]) => twMerge(clsx(inputs));
