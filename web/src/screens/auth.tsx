import { zodResolver } from "@hookform/resolvers/zod";
import { useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { KeyRound } from "lucide-react";
import { AnimatePresence } from "motion/react";
import * as motion from "motion/react-m";
import { useEffect, useMemo, useRef, useState } from "react";
import { useForm } from "react-hook-form";
import { z } from "zod";
import { useApi } from "@/api/context";
import { describeError, isApiError } from "@/api/errors";
import { qk, useSession } from "@/api/queries";
import type { FlowStep, SessionView, SignInIntent } from "@/api/types";
import { AuthLayout } from "@/components/auth-layout";
import { m } from "@/i18n";
import { usePageTitle } from "@/lib/use-page-title";
import { Button } from "@/ui/button";
import { Field } from "@/ui/field";
import { Input } from "@/ui/input";
import { OtpInput } from "@/ui/otp-input";
import { Checkbox } from "@/ui/toggle";
import { ReturnPanel } from "./return";

type Step = "email" | "code" | "details" | "terms";

export interface AuthScreenProps {
	mode: SignInIntent;
	/** Where to go after sign-in: a same-origin path, checked by the route. */
	next?: string;
}

const EASE_OUT = [0.23, 1, 0.32, 1] as const;
const link =
	"font-medium text-fg underline decoration-line-strong underline-offset-4 hover:decoration-fg";

/**
 * Sign in and create account. The cloud owns the state machine: every request answers with the
 * session view, and `flow.step` says which step to show. The screen keeps nothing about the flow
 * but one choice, "use a different e-mail", which only reopens the first step.
 */
export function AuthScreen({ mode, next }: AuthScreenProps) {
	usePageTitle(mode === "sign_in" ? m.auth_signin_link() : m.auth_create_button());
	const api = useApi();
	const qc = useQueryClient();
	const navigate = useNavigate();
	const session = useSession();
	const view = session.data;
	const [reopen, setReopen] = useState(false);
	const [passkeyBusy, setPasskeyBusy] = useState(false);
	const [passkeyError, setPasskeyError] = useState<string | null>(null);
	const [support, setSupport] = useState({ available: false, conditional: false });
	const direction = useRef(1);

	const step: FlowStep = view?.flow.step ?? "email";
	const shown: Step = reopen
		? "email"
		: step === "code" || step === "details" || step === "terms"
			? step
			: "email";

	const accept = (v: SessionView) => {
		direction.current = 1;
		setReopen(false);
		qc.setQueryData(qk.session, v);
	};
	const acceptRef = useRef(accept);
	acceptRef.current = accept;

	// Where the flow ends: the account, the launcher, or the ban statement.
	const redirect = view?.flow.redirect;
	useEffect(() => {
		if (step === "done") void navigate({ to: next ?? "/account" });
		else if (step === "banned") void navigate({ to: "/banned" });
		else if (step === "handoff" && redirect) {
			const id = window.setTimeout(() => window.location.assign(redirect), 900);
			return () => window.clearTimeout(id);
		}
	}, [step, redirect, next, navigate]);

	// Passkey autofill: the browser offers saved passkeys in the e-mail field's suggestions.
	useEffect(() => {
		if (mode !== "sign_in" || shown !== "email") return;
		const ctrl = new AbortController();
		let live = true;
		void api.auth
			.passkeySupport()
			.then((s) => {
				if (!live) return;
				setSupport(s);
				if (!s.conditional) return;
				return api.auth
					.signInWithPasskey({ conditional: true, signal: ctrl.signal })
					.then((v) => live && acceptRef.current(v));
			})
			.catch(() => {
				// closed, aborted or unsupported: the e-mail form still works
			});
		return () => {
			live = false;
			ctrl.abort();
		};
	}, [api, mode, shown]);

	const signInWithPasskey = async () => {
		setPasskeyBusy(true);
		setPasskeyError(null);
		try {
			accept(await api.auth.signInWithPasskey({ conditional: false }));
		} catch (e) {
			if (isApiError(e) && e.code === "passkey_cancelled")
				setPasskeyError(m.auth_passkey_cancelled());
			else setPasskeyError(describeError(e));
		} finally {
			setPasskeyBusy(false);
		}
	};

	if (step === "handoff") {
		return (
			<AuthLayout>
				<ReturnPanel handle={view?.account?.handle} />
			</AuthLayout>
		);
	}

	const heading =
		shown === "email"
			? mode === "sign_in"
				? m.auth_signin_title()
				: m.auth_create_title()
			: shown === "code"
				? m.auth_code_title()
				: shown === "details"
					? m.auth_details_title()
					: m.auth_terms_title();

	return (
		<AuthLayout
			footer={
				shown === "email" ? (
					mode === "sign_in" ? (
						<p className="text-body text-fg-muted">
							{m.auth_new_here()}{" "}
							<Link to="/create" search={{ next }} className={link}>
								{m.auth_create_link()}
							</Link>
						</p>
					) : (
						<p className="text-body text-fg-muted">
							{m.auth_have_account()}{" "}
							<Link to="/sign-in" search={{ next }} className={link}>
								{m.auth_signin_link()}
							</Link>
						</p>
					)
				) : null
			}
		>
			<AnimatePresence mode="wait" initial={false}>
				<motion.div
					key={shown}
					initial={{ opacity: 0, x: 14 * direction.current }}
					animate={{ opacity: 1, x: 0, transition: { duration: 0.2, ease: EASE_OUT } }}
					exit={{
						opacity: 0,
						x: -8 * direction.current,
						transition: { duration: 0.12, ease: EASE_OUT },
					}}
					className="flex flex-col gap-6"
				>
					<div>
						<h1 className="text-heading font-semibold text-fg">{heading}</h1>
						<p className="mt-2 text-body text-fg-muted">
							{shown === "email" &&
								(view?.authorize
									? m.auth_launcher_waiting({ device: view.authorize.device })
									: mode === "sign_in"
										? m.auth_signin_lead()
										: m.auth_create_lead())}
							{shown === "code" && m.auth_code_lead({ email: view?.flow.email ?? "" })}
							{shown === "details" && m.auth_details_lead({ email: view?.flow.email ?? "" })}
							{shown === "terms" && m.auth_terms_lead()}
						</p>
					</div>

					{shown === "email" ? (
						<EmailStep
							mode={mode}
							passkey={support.available && mode === "sign_in"}
							passkeyBusy={passkeyBusy}
							passkeyError={passkeyError}
							onPasskey={signInWithPasskey}
							onView={accept}
						/>
					) : null}
					{shown === "code" ? (
						<CodeStep
							notice={view?.notice ?? null}
							onBack={() => {
								direction.current = -1;
								setReopen(true);
							}}
							onView={accept}
						/>
					) : null}
					{shown === "details" ? <DetailsStep terms={view?.flow.terms} onView={accept} /> : null}
					{shown === "terms" ? <TermsStep terms={view?.flow.terms} onView={accept} /> : null}
				</motion.div>
			</AnimatePresence>
		</AuthLayout>
	);
}

function EmailStep({
	mode,
	passkey,
	passkeyBusy,
	passkeyError,
	onPasskey,
	onView,
}: {
	mode: SignInIntent;
	passkey: boolean;
	passkeyBusy: boolean;
	passkeyError: string | null;
	onPasskey: () => void;
	onView: (v: SessionView) => void;
}) {
	const api = useApi();
	const schema = useMemo(() => z.object({ email: z.email({ error: m.auth_email_invalid() }) }), []);
	const {
		register,
		handleSubmit,
		setError,
		formState: { errors, isSubmitting },
	} = useForm<z.infer<typeof schema>>({
		resolver: zodResolver(schema),
		defaultValues: { email: "" },
	});

	const submit = handleSubmit(async ({ email }) => {
		try {
			onView(await api.auth.start({ email, intent: mode }));
		} catch (e) {
			setError("email", { message: describeError(e) });
		}
	});

	return (
		<div className="flex flex-col gap-5">
			<form onSubmit={submit} noValidate className="flex flex-col gap-4">
				<Field label={m.auth_email_label()} error={errors.email?.message}>
					<Input
						type="email"
						inputMode="email"
						// "webauthn" lets the browser fill a saved passkey from this field
						autoComplete="username webauthn"
						autoCapitalize="none"
						spellCheck={false}
						placeholder={m.auth_email_placeholder()}
						autoFocus
						{...register("email")}
					/>
				</Field>
				<Button type="submit" variant="primary" size="lg" loading={isSubmitting}>
					{m.auth_send_code()}
				</Button>
			</form>
			{passkey ? (
				<>
					<div className="flex items-center gap-3 text-small text-fg-muted" aria-hidden>
						<span className="h-px flex-1 bg-line" />
						{m.auth_or()}
						<span className="h-px flex-1 bg-line" />
					</div>
					<div className="flex flex-col gap-2">
						<Button variant="secondary" size="lg" onClick={onPasskey} loading={passkeyBusy}>
							<KeyRound aria-hidden />
							{m.auth_passkey_button()}
						</Button>
						{passkeyError ? (
							<p role="alert" className="text-small text-danger-text">
								{passkeyError}
							</p>
						) : null}
					</div>
				</>
			) : null}
		</div>
	);
}

const RESEND_AFTER = 30;

function useCountdown(from: number) {
	const [left, setLeft] = useState(() => Math.max(0, Math.ceil((from - Date.now()) / 1000)));
	useEffect(() => {
		const tick = () => setLeft(Math.max(0, Math.ceil((from - Date.now()) / 1000)));
		tick();
		const id = window.setInterval(tick, 1000);
		return () => window.clearInterval(id);
	}, [from]);
	return left;
}

function CodeStep({
	notice,
	onBack,
	onView,
}: {
	notice: string | null;
	onBack: () => void;
	onView: (v: SessionView) => void;
}) {
	const api = useApi();
	const [code, setCode] = useState("");
	const [error, setError] = useState<string | null>(null);
	const [busy, setBusy] = useState(false);
	const [resending, setResending] = useState(false);
	const [resendAt, setResendAt] = useState(() => Date.now() + RESEND_AFTER * 1000);
	const left = useCountdown(resendAt);

	const verify = async (value: string) => {
		if (value.length < 6 || busy) return;
		setBusy(true);
		setError(null);
		try {
			onView(await api.auth.verifyCode(value));
		} catch (e) {
			setCode("");
			setError(describeError(e));
		} finally {
			setBusy(false);
		}
	};

	const resend = async () => {
		setResending(true);
		setError(null);
		try {
			onView(await api.auth.resendCode());
			setResendAt(Date.now() + RESEND_AFTER * 1000);
			setCode("");
		} catch (e) {
			setError(describeError(e));
		} finally {
			setResending(false);
		}
	};

	return (
		<form
			className="flex flex-col gap-5"
			onSubmit={(e) => {
				e.preventDefault();
				void verify(code);
			}}
		>
			<Field label={m.auth_code_label()} error={error}>
				<OtpInput
					value={code}
					onChange={setCode}
					onComplete={(v) => void verify(v)}
					invalid={Boolean(error)}
					disabled={busy}
					autoFocus
				/>
			</Field>
			<Button type="submit" variant="primary" size="lg" loading={busy} disabled={code.length < 6}>
				{m.common_continue()}
			</Button>
			<div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
				<Button variant="ghost" type="button" onClick={onBack}>
					{m.auth_other_email()}
				</Button>
				<Button
					variant="ghost"
					type="button"
					onClick={resend}
					loading={resending}
					disabled={left > 0}
					className="nums"
				>
					{left > 0 ? m.auth_resend_in({ seconds: left }) : m.auth_resend()}
				</Button>
			</div>
			<p role="status" className="-mt-2 min-h-5 text-small text-fg-muted">
				{notice ?? ""}
			</p>
		</form>
	);
}

function TermsLabel({ terms }: { terms?: { version: string; url: string | null } }) {
	if (!terms?.url) return <>{m.auth_terms_accept_plain()}</>;
	return (
		<>
			{m.auth_terms_accept_pre()}{" "}
			<a href={terms.url} target="_blank" rel="noopener noreferrer" className={link}>
				{m.auth_terms_link()}
				<span className="sr-only"> {m.common_new_tab()}</span>
			</a>
			{terms.version ? ` (${m.auth_terms_version({ version: terms.version })})` : null}.
		</>
	);
}

function DetailsStep({
	terms,
	onView,
}: {
	terms?: { version: string; url: string | null };
	onView: (v: SessionView) => void;
}) {
	const api = useApi();
	const schema = useMemo(
		() =>
			z.object({
				handle: z
					.string()
					.trim()
					.regex(/^[A-Za-z][A-Za-z0-9_-]{2,15}$/, { error: m.auth_handle_rules() }),
				ageConfirmed: z.boolean().refine((v) => v, { error: m.auth_age_required() }),
				termsAccepted: terms
					? z.boolean().refine((v) => v, { error: m.auth_terms_required() })
					: z.boolean(),
			}),
		[terms],
	);
	const {
		register,
		handleSubmit,
		setError,
		watch,
		setValue,
		formState: { errors, isSubmitting },
	} = useForm<z.infer<typeof schema>>({
		resolver: zodResolver(schema),
		defaultValues: { handle: "", ageConfirmed: false, termsAccepted: false },
	});

	const submit = handleSubmit(async (values) => {
		try {
			onView(
				await api.auth.submitDetails({
					...values,
					termsAccepted: terms ? values.termsAccepted : true,
				}),
			);
		} catch (e) {
			const map: Record<string, "handle" | "ageConfirmed" | "termsAccepted"> = {
				handle: "handle",
				age_confirmed: "ageConfirmed",
				terms_accepted: "termsAccepted",
			};
			if (isApiError(e)) {
				const field = Object.keys(e.fields)[0];
				const target = field ? map[field] : undefined;
				if (field && target) {
					setError(target, { message: e.fields[field] });
					return;
				}
			}
			setError("handle", { message: describeError(e) });
		}
	});

	return (
		<form onSubmit={submit} noValidate className="flex flex-col gap-5">
			<Field
				label={m.auth_handle_label()}
				description={m.auth_handle_hint()}
				error={errors.handle?.message}
			>
				<Input
					autoComplete="nickname"
					autoCapitalize="none"
					spellCheck={false}
					className="font-mono"
					autoFocus
					{...register("handle")}
				/>
			</Field>
			<fieldset className="flex flex-col gap-4">
				<legend className="sr-only">{m.auth_terms_title()}</legend>
				{terms ? (
					<CheckRow
						checked={watch("termsAccepted")}
						onChange={(v) => setValue("termsAccepted", v, { shouldValidate: true })}
						error={errors.termsAccepted?.message}
					>
						<TermsLabel terms={terms} />
					</CheckRow>
				) : null}
				<CheckRow
					checked={watch("ageConfirmed")}
					onChange={(v) => setValue("ageConfirmed", v, { shouldValidate: true })}
					error={errors.ageConfirmed?.message}
					hint={m.auth_age_hint()}
				>
					{m.auth_age_label()}
				</CheckRow>
			</fieldset>
			<Button type="submit" variant="primary" size="lg" loading={isSubmitting}>
				{m.auth_create_button()}
			</Button>
		</form>
	);
}

function CheckRow({
	checked,
	onChange,
	error,
	hint,
	children,
}: {
	checked: boolean;
	onChange: (v: boolean) => void;
	error?: string;
	hint?: string;
	children: React.ReactNode;
}) {
	return (
		<div>
			{/* biome-ignore lint/a11y/noLabelWithoutControl: the Base UI checkbox inside is the control */}
			<label className="flex items-start gap-3 text-body">
				<Checkbox checked={checked} onCheckedChange={onChange} className="mt-0.5" />
				<span>
					{children}
					{hint ? <span className="mt-0.5 block text-small text-fg-muted">{hint}</span> : null}
				</span>
			</label>
			{error ? (
				<p role="alert" className="mt-1.5 ps-[1.875rem] text-small text-danger-text">
					{error}
				</p>
			) : null}
		</div>
	);
}

/** The terms changed since the player last agreed. */
function TermsStep({
	terms,
	onView,
}: {
	terms?: { version: string; url: string | null };
	onView: (v: SessionView) => void;
}) {
	const api = useApi();
	const [accepted, setAccepted] = useState(false);
	const [error, setError] = useState<string | null>(null);
	const [busy, setBusy] = useState(false);

	const submit = async () => {
		if (!accepted) {
			setError(m.auth_terms_required());
			return;
		}
		setBusy(true);
		setError(null);
		try {
			onView(await api.auth.acceptTerms());
		} catch (e) {
			setError(describeError(e));
		} finally {
			setBusy(false);
		}
	};

	return (
		<form
			className="flex flex-col gap-5"
			onSubmit={(e) => {
				e.preventDefault();
				void submit();
			}}
		>
			<CheckRow checked={accepted} onChange={setAccepted} error={error ?? undefined}>
				<TermsLabel terms={terms} />
			</CheckRow>
			<Button type="submit" variant="primary" size="lg" loading={busy}>
				{m.auth_terms_continue()}
			</Button>
		</form>
	);
}
