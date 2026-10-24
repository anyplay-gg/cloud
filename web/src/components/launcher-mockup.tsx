import { Gamepad2, Library, Settings, Trophy, Users } from "lucide-react";
import { m } from "@/i18n";

/**
 * A stand-in for the launcher window: its chrome, its rail, a library of neutral tiles and a
 * friends panel. Every tile is a grey placeholder. There is no game art, no screenshot and no
 * platform mark on purpose. It is decoration, so it is hidden from assistive technology.
 *
 * Surface recipe: three layers of soft shadow (a hairline, a mid lift, an ambient fall-off), and
 * concentric radii: 20 outer = 12 inner + 8 padding.
 */
export function LauncherMockup() {
	return (
		<div
			aria-hidden
			className="rounded-[1.25rem] bg-[rgb(255_255_255/0.06)] p-2 shadow-[0_0_0_1px_rgb(255_255_255/0.1),0_2px_4px_rgb(0_0_0/0.12),0_12px_24px_-4px_rgb(0_0_0/0.2),0_40px_80px_-12px_rgb(0_0_0/0.45)]"
		>
			<div className="overflow-hidden rounded-xl bg-[#0b0d12] text-[#e9e9ea] shadow-[inset_0_1px_0_rgb(255_255_255/0.06)]">
				<div className="flex h-9 items-center gap-1.5 border-b border-[rgb(255_255_255/0.07)] px-3.5">
					<span className="size-2.5 rounded-full bg-[rgb(255_255_255/0.16)]" />
					<span className="size-2.5 rounded-full bg-[rgb(255_255_255/0.16)]" />
					<span className="size-2.5 rounded-full bg-[rgb(255_255_255/0.16)]" />
					<span className="ms-3 text-caption text-[rgb(255_255_255/0.5)]">AnyPlay</span>
				</div>
				<div className="grid grid-cols-[3rem_1fr] sm:grid-cols-[3.25rem_1fr_11rem]">
					<div className="flex flex-col items-center gap-3 border-e border-[rgb(255_255_255/0.07)] py-4 text-[rgb(255_255_255/0.5)]">
						<span className="flex size-8 items-center justify-center rounded-lg bg-white text-black">
							<Library className="size-4" strokeWidth={1.75} />
						</span>
						<Users className="size-4" strokeWidth={1.75} />
						<Trophy className="size-4" strokeWidth={1.75} />
						<Settings className="mt-auto size-4" strokeWidth={1.75} />
					</div>
					<div className="p-4">
						<p className="text-small font-medium">{m.mock_library()}</p>
						<div className="mt-3 grid grid-cols-3 gap-2.5">
							{[0, 1, 2, 3, 4, 5].map((i) => (
								<div
									key={i}
									className="aspect-[4/5] rounded-[0.625rem] bg-[linear-gradient(160deg,rgb(255_255_255/0.1),rgb(255_255_255/0.04))] shadow-[inset_0_0_0_1px_rgb(255_255_255/0.07)]"
								>
									<span className="flex h-full items-end p-2">
										<span className="h-1 w-1/2 rounded-full bg-[rgb(255_255_255/0.18)]" />
									</span>
								</div>
							))}
						</div>
						<div className="mt-4 flex items-center gap-3">
							<Gamepad2 className="size-4 text-[rgb(255_255_255/0.5)]" strokeWidth={1.75} />
							<span className="h-1.5 flex-1 overflow-hidden rounded-full bg-[rgb(255_255_255/0.14)]">
								<span className="block h-full w-2/3 rounded-full bg-[#4f94ff] shadow-[0_0_6px_rgb(79_148_255/0.6)]" />
							</span>
						</div>
					</div>
					<div className="hidden border-s border-[rgb(255_255_255/0.07)] p-4 sm:block">
						<p className="text-small font-medium">{m.mock_friends()}</p>
						<ul className="mt-3 space-y-3">
							{[
								["#5cebbd", "w-20"],
								["#ff8066", "w-16"],
								["#a885ff", "w-24"],
							].map(([c, w]) => (
								<li key={c} className="flex items-center gap-2.5">
									<span className="relative size-7 rounded-full" style={{ background: c }}>
										<span className="absolute -bottom-px -end-px size-2.5 rounded-full bg-[#4f94ff] ring-2 ring-[#0b0d12]" />
									</span>
									<span className={`h-2 rounded-full bg-[rgb(255_255_255/0.2)] ${w}`} />
								</li>
							))}
						</ul>
						<div className="mt-5 rounded-lg bg-[rgb(255_255_255/0.06)] p-2.5">
							<span className="block h-2 w-3/4 rounded-full bg-[rgb(255_255_255/0.2)]" />
							<span className="mt-2 block h-2 w-1/2 rounded-full bg-[rgb(255_255_255/0.12)]" />
						</div>
					</div>
				</div>
			</div>
		</div>
	);
}
