import { Dialog as BaseDialog } from "@base-ui/react/dialog";
import { useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import {
	ChevronsUpDown,
	Database,
	ExternalLink,
	EyeOff,
	LayoutGrid,
	LogOut,
	Menu as MenuIcon,
	Monitor,
	Moon,
	Palette,
	ShieldCheck,
	Sun,
	Trophy,
	UserRound,
	Users,
	X,
} from "lucide-react";
import { type ReactNode, useState } from "react";
import { useAction, useFriends, useSession } from "@/api/queries";
import type { AccentKey, Me } from "@/api/types";
import { ACCENT_KEYS, type ThemeChoice, useAppearance } from "@/features/appearance";
import { m } from "@/i18n";
import { cn } from "@/lib/cn";
import { Avatar } from "@/ui/avatar";
import {
	Menu,
	MenuContent,
	MenuItem,
	MenuLabel,
	MenuRadioGroup,
	MenuRadioItem,
	MenuSeparator,
	MenuTrigger,
} from "@/ui/menu";
import { Tooltip } from "@/ui/tooltip";
import { Wordmark } from "./logo";

interface NavEntry {
	to:
		| "/account"
		| "/account/profile"
		| "/account/friends"
		| "/account/trophies"
		| "/account/security"
		| "/account/privacy"
		| "/account/data";
	label: () => string;
	icon: typeof LayoutGrid;
	exact?: boolean;
}

const primary: NavEntry[] = [
	{ to: "/account", label: () => m.nav_overview(), icon: LayoutGrid, exact: true },
	{ to: "/account/profile", label: () => m.nav_profile(), icon: UserRound },
	{ to: "/account/friends", label: () => m.nav_friends(), icon: Users },
	{ to: "/account/trophies", label: () => m.nav_trophies(), icon: Trophy },
];
const account: NavEntry[] = [
	{ to: "/account/security", label: () => m.nav_security(), icon: ShieldCheck },
	{ to: "/account/privacy", label: () => m.nav_privacy(), icon: EyeOff },
	{ to: "/account/data", label: () => m.nav_data(), icon: Database },
];

function NavItem({
	entry,
	badge,
	onNavigate,
}: {
	entry: NavEntry;
	badge?: number;
	onNavigate?: () => void;
}) {
	const Icon = entry.icon;
	return (
		<Link
			to={entry.to}
			activeOptions={{ exact: entry.exact }}
			onClick={onNavigate}
			className={cn(
				"group press flex h-9 items-center gap-2.5 rounded-md px-2.5 text-body font-medium text-fg-muted outline-offset-0 coarse:h-11",
				"pointer-hover:hover:bg-hover pointer-hover:hover:text-fg",
				"data-[status=active]:bg-hover data-[status=active]:text-fg",
			)}
			activeProps={{ "aria-current": "page" }}
		>
			<Icon
				aria-hidden
				strokeWidth={1.75}
				className="size-4 shrink-0 group-data-[status=active]:text-accent-text"
			/>
			<span className="flex-1 truncate">{entry.label()}</span>
			{badge ? (
				<span className="nums flex h-5 min-w-5 items-center justify-center rounded-full bg-accent-soft px-1.5 text-caption font-semibold text-accent-text">
					{badge}
					<span className="sr-only"> {m.nav_pending()}</span>
				</span>
			) : null}
		</Link>
	);
}

function NavLists({ onNavigate }: { onNavigate?: () => void }) {
	const friends = useFriends();
	const requests = friends.data?.incoming.length ?? 0;
	return (
		<nav aria-label={m.nav_main_label()} className="flex flex-col gap-0.5 px-3 py-2">
			{primary.map((e) => (
				<NavItem
					key={e.to}
					entry={e}
					onNavigate={onNavigate}
					badge={e.to === "/account/friends" ? requests : undefined}
				/>
			))}
			<p className="px-2.5 pb-1 pt-5 text-caption font-medium text-fg-muted">
				{m.nav_account_group()}
			</p>
			{account.map((e) => (
				<NavItem key={e.to} entry={e} onNavigate={onNavigate} />
			))}
		</nav>
	);
}

const THEMES: { value: ThemeChoice; label: () => string; icon: typeof Sun }[] = [
	{ value: "system", label: () => m.theme_system(), icon: Monitor },
	{ value: "dark", label: () => m.theme_dark(), icon: Moon },
	{ value: "light", label: () => m.theme_light(), icon: Sun },
];

const ACCENT_LABEL: Record<AccentKey, () => string> = {
	blue: () => m.accent_blue(),
	mint: () => m.accent_mint(),
	coral: () => m.accent_coral(),
	violet: () => m.accent_violet(),
	gold: () => m.accent_gold(),
	white: () => m.accent_white(),
};

export function AppearanceMenu({ side = "bottom" }: { side?: "top" | "bottom" }) {
	const { theme, setTheme, accent, setAccent } = useAppearance();
	return (
		<Menu>
			<Tooltip label={m.appearance()} side={side === "top" ? "top" : "bottom"}>
				<MenuTrigger
					aria-label={m.appearance()}
					className="press flex size-9 shrink-0 items-center justify-center rounded-md text-fg-muted outline-offset-0 pointer-hover:hover:bg-hover pointer-hover:hover:text-fg data-[popup-open]:bg-hover data-[popup-open]:text-fg coarse:size-11"
				>
					<Palette aria-hidden strokeWidth={1.75} className="size-4" />
				</MenuTrigger>
			</Tooltip>
			<MenuContent align={side === "top" ? "start" : "end"} side={side}>
				<MenuLabel>{m.theme_label()}</MenuLabel>
				<MenuRadioGroup value={theme} onValueChange={(v) => setTheme(v as ThemeChoice)}>
					{THEMES.map((t) => (
						<MenuRadioItem
							key={t.value}
							value={t.value}
							leading={<t.icon aria-hidden strokeWidth={1.75} />}
						>
							{t.label()}
						</MenuRadioItem>
					))}
				</MenuRadioGroup>
				<MenuSeparator />
				<MenuLabel>{m.accent_label()}</MenuLabel>
				<MenuRadioGroup value={accent} onValueChange={(v) => setAccent(v as AccentKey)}>
					{ACCENT_KEYS.map((k) => (
						<MenuRadioItem
							key={k}
							value={k}
							leading={
								<span
									aria-hidden
									className="size-3.5 shrink-0 rounded-full shadow-[inset_0_0_0_1px_rgb(0_0_0/0.18)]"
									style={{ background: `var(--swatch-${k})` }}
								/>
							}
						>
							{ACCENT_LABEL[k]()}
						</MenuRadioItem>
					))}
				</MenuRadioGroup>
			</MenuContent>
		</Menu>
	);
}

function useSignOut() {
	const qc = useQueryClient();
	const navigate = useNavigate();
	return useAction((_: void, api) => api.auth.signOut(), {
		success: m.toast_signed_out(),
		onSuccess: () => {
			qc.clear();
			void navigate({ to: "/sign-in", search: {} });
		},
	});
}

export function AccountMenu({ me, variant }: { me: Me; variant: "sidebar" | "bar" }) {
	const signOut = useSignOut();
	return (
		<Menu>
			<MenuTrigger
				aria-label={m.account_menu({ name: me.displayName })}
				className={cn(
					"press flex items-center gap-2.5 rounded-md text-start outline-offset-0 data-[popup-open]:bg-hover",
					variant === "sidebar"
						? "h-11 min-w-0 flex-1 px-2 pointer-hover:hover:bg-hover"
						: "size-9 justify-center coarse:size-11",
				)}
			>
				<Avatar
					name={me.displayName}
					accent={me.accent}
					size={variant === "sidebar" ? "sm" : "sm"}
				/>
				{variant === "sidebar" ? (
					<>
						<span className="flex min-w-0 flex-1 flex-col leading-tight">
							<span className="truncate text-body font-medium text-fg">{me.displayName}</span>
							<span className="truncate font-mono text-caption text-fg-muted">@{me.handle}</span>
						</span>
						<ChevronsUpDown
							aria-hidden
							className="size-4 shrink-0 text-fg-muted"
							strokeWidth={1.75}
						/>
					</>
				) : null}
			</MenuTrigger>
			<MenuContent
				align={variant === "sidebar" ? "start" : "end"}
				side={variant === "sidebar" ? "top" : "bottom"}
			>
				<div className="px-2.5 pb-1.5 pt-2">
					<p className="truncate text-body font-medium text-fg">{me.displayName}</p>
					<p className="truncate text-small text-fg-muted">{me.email}</p>
				</div>
				<MenuSeparator />
				<MenuItem render={<Link to="/u/$name" params={{ name: me.handle }} />}>
					<ExternalLink aria-hidden strokeWidth={1.75} />
					{m.menu_view_profile()}
				</MenuItem>
				<MenuItem onClick={() => signOut.mutate()}>
					<LogOut aria-hidden strokeWidth={1.75} />
					{m.sign_out()}
				</MenuItem>
			</MenuContent>
		</Menu>
	);
}

function MobileNav({ me }: { me: Me }) {
	const [open, setOpen] = useState(false);
	return (
		<BaseDialog.Root open={open} onOpenChange={setOpen}>
			<BaseDialog.Trigger
				aria-label={m.menu_open()}
				className="press -ms-2 flex size-9 items-center justify-center rounded-md text-fg-muted outline-offset-0 pointer-hover:hover:bg-hover pointer-hover:hover:text-fg coarse:size-11"
			>
				<MenuIcon aria-hidden strokeWidth={1.75} className="size-5" />
			</BaseDialog.Trigger>
			<BaseDialog.Portal>
				<BaseDialog.Backdrop className="backdrop fixed inset-0 z-40 bg-scrim" />
				<BaseDialog.Popup className="sheet-pop fixed inset-y-0 start-0 z-50 flex w-[min(20rem,calc(100vw-3rem))] flex-col bg-surface shadow-dialog outline-none">
					<div className="flex h-14 items-center justify-between ps-4 pe-2">
						<BaseDialog.Title render={<span />}>
							<Wordmark />
						</BaseDialog.Title>
						<BaseDialog.Close
							aria-label={m.common_close()}
							className="press flex size-9 items-center justify-center rounded-md text-fg-muted pointer-hover:hover:bg-hover coarse:size-11"
						>
							<X aria-hidden className="size-4" />
						</BaseDialog.Close>
					</div>
					<div className="min-h-0 flex-1 overflow-y-auto">
						<NavLists onNavigate={() => setOpen(false)} />
					</div>
					<div className="flex items-center gap-1 border-t border-line p-3">
						<span className="flex min-w-0 flex-1 items-center gap-2.5 px-2">
							<Avatar name={me.displayName} accent={me.accent} size="sm" />
							<span className="flex min-w-0 flex-col leading-tight">
								<span className="truncate text-body font-medium">{me.displayName}</span>
								<span className="truncate font-mono text-caption text-fg-muted">@{me.handle}</span>
							</span>
						</span>
					</div>
				</BaseDialog.Popup>
			</BaseDialog.Portal>
		</BaseDialog.Root>
	);
}

/** Sidebar on wide screens, a compact bar and a sheet on phones. */
export function AppShell({ children }: { children: ReactNode }) {
	const session = useSession();
	const me = session.data?.account;
	return (
		<div className="min-h-dvh">
			<a
				href="#main"
				className="sr-only z-[80] rounded-md bg-ink px-3 py-2 text-body font-medium text-on-ink focus:not-sr-only focus:fixed focus:start-3 focus:top-3"
			>
				{m.skip_to_content()}
			</a>

			<aside className="fixed inset-y-0 start-0 z-30 hidden w-[var(--sidebar-w)] flex-col border-e border-line bg-page nav:flex">
				<div className="flex h-14 items-center px-5">
					<Link
						to="/account"
						aria-label={m.nav_home_label()}
						className="rounded-md outline-offset-4"
					>
						<Wordmark />
					</Link>
				</div>
				<div className="min-h-0 flex-1 overflow-y-auto">
					<NavLists />
				</div>
				{me ? (
					<div className="flex items-center gap-1 border-t border-line p-3">
						<AccountMenu me={me} variant="sidebar" />
						<AppearanceMenu side="top" />
					</div>
				) : null}
			</aside>

			<header className="sticky top-0 z-30 flex h-[var(--topbar-h)] items-center justify-between gap-2 border-b border-line bg-page/85 px-4 backdrop-blur-md nav:hidden">
				<div className="flex items-center gap-2">
					{me ? <MobileNav me={me} /> : null}
					<Link
						to="/account"
						aria-label={m.nav_home_label()}
						className="rounded-md outline-offset-4"
					>
						<Wordmark />
					</Link>
				</div>
				<div className="flex items-center gap-1">
					<AppearanceMenu />
					{me ? <AccountMenu me={me} variant="bar" /> : null}
				</div>
			</header>

			<main
				id="main"
				tabIndex={-1}
				className="px-4 pb-20 pt-8 outline-none sm:px-8 nav:ps-[calc(var(--sidebar-w)+2.5rem)] nav:pe-10 nav:pt-12"
			>
				{children}
			</main>
		</div>
	);
}
