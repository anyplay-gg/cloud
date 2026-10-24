import { useEffect, useState } from "react";
import { cn } from "@/lib/cn";

/**
 * A number whose digits roll up from zero to their place (the rolling-counter pattern from
 * Bencho, written again). Each digit is a strip of 0 to 9 moved with transform. The visible
 * strips are hidden from assistive technology; the plain number is the accessible name.
 * Under reduced motion the digits simply sit in place.
 */
export function RollingNumber({
	value,
	className,
	suffix,
}: {
	value: number;
	className?: string;
	suffix?: string;
}) {
	const [shown, setShown] = useState(0);
	useEffect(() => {
		const id = requestAnimationFrame(() => setShown(value));
		return () => cancelAnimationFrame(id);
	}, [value]);
	const target = String(Math.max(0, Math.round(value)));
	// a digit is known by its place (ones, tens, ...), which is what stays when the value changes
	const places = target.split("").map((_, i, all) => `place-${all.length - i}`);
	const current = String(Math.max(0, Math.round(shown))).padStart(target.length, "0");
	return (
		<span className={cn("nums inline-flex", className)}>
			<span className="sr-only">{`${target}${suffix ?? ""}`}</span>
			<span aria-hidden className="inline-flex">
				{places.map((place, i) => {
					const d = Number(current[i] ?? 0);
					return (
						<span
							key={place}
							className="inline-block h-[1em] overflow-hidden leading-none [line-height:1em]"
						>
							<span
								className="roll-strip flex flex-col"
								style={{ transform: `translateY(${-d * 10}%)` }}
							>
								{[0, 1, 2, 3, 4, 5, 6, 7, 8, 9].map((n) => (
									<span key={n} className="block h-[1em] leading-none">
										{n}
									</span>
								))}
							</span>
						</span>
					);
				})}
				{suffix ? <span>{suffix}</span> : null}
			</span>
		</span>
	);
}
