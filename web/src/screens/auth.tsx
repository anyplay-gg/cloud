import { zodResolver } from "@hookform/resolvers/zod";
import { usePageTitle } from "@/lib/use-page-title";
import { useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { KeyRound } from "lucide-react";
import { AnimatePresence } from "motion/react";
import * as motion from "motion/react-m";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useForm } from "react-hook-form";
import { z } from "zod";
import { useApi } from "@/api/context";
import { describeError, isApiError } from "@/api/errors";
import { qk } from "@/api/queries";
import type { AuthProgress, AuthStep, SignInIntent } from "@/api/types";
import { AuthLayout } from "@/components/auth-layout";
import { m } from "@/i18n";
import { Button } from "@/ui/button";
import { Field } from "@/ui/field";
import { Input } from "@/ui/input";
import { OtpInput } from "@/ui/otp-input";
import { Checkbox } from "@/ui/toggle";

type Step = "email" | "code" | "details" | "terms";

export interface AuthScreenProps {
	mode: SignInIntent;
	/** Where to go after sign-in: a same-origin path, checked by the route. */
	next?: string;
	/** The launcher sent the player here; finish on the "return to AnyPlay" page. */
	handoff?: boolean;
}

const EASE_OUT = [0.23, 1, 0.32, 1] as const;

export function AuthScreen({ mode, next, handoff }: AuthScreenProps) {
	usePageTitle(mode === "sign_in" ? m.auth_signin_link() : m.auth_create_button());
	const api = useApi();
	const qc = useQueryClient();
	const navigate = useNavigate();
	const [step, setStep] = useState<Step>("email");
	const [email, setEmail] = useState("");
	const [hint, setHint] = useState("");
	const [resendAt, setResendAt] = useState(0);
	const [passkeyBusy, setPasskeyBusy] = useState(false);
	const [passkeyError, setPasskeyError] = useState<string | null>(null);
	const [support, setSupport] = useState({ available: false, conditional: false });
	const direction = useRef(1);

	const finish = useCallback(async () => {
		await qc.invalidateQueries({ queryKey: qk.session });
		if (handoff) await navigate({ to: "/return" });
		else await navigate({ to: next ?? "/account" });
	}, [qc, navigate, next, handoff]);

	const go = (to: Step, dir = 1) => {
		direction.current = dir;
		setStep(to);
	};
	const advance = (p: AuthProgress) => {
		const map: Record<AuthStep, Step | null> = {
			code: "code",
			details: "details",
			terms: "terms",
			done: null,
		};
		const to = map[p.next];
		if (to) go(to);
		else void finish();
	};

	// Passkey autofill: the browser offers saved passkeys in the e-mail field's suggestions.
	useEffect(() => {
		if (mode !== "sign_in" || step !== "email") return;
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
					.then((p) => live && advance(p));
			})
			.catch(() => {
				// closed, aborted or unsupported: the e-mail form still works
			});
		return () => {
			live = false;
			ctrl.abort();
		};
		// advance changes every render; the effect only depends on what starts a ceremony
		// biome-ignore lint/correctness/useExhaustiveDependencies: see above
	}, [api, mode, step]);

	const signInWithPasskey = async () => {
		setPasskeyBusy(true);
		setPasskeyError(null);
		try {
			advance(await api.auth.signInWithPasskey({ conditional: false }));
		} catch (e) {
			if (isApiError(e) && e.code === "passkey_cancelled")
				setPasskeyError(m.auth_passkey_cancelled());
			else setPasskeyError(describeError(e));
		} finally {
			setPasskeyBusy(false);
		}
	};

	const heading =
		step === "email"
			? mode === "sign_in"
				? m.auth_signin_title()
				: m.auth_create_title()
			: step === "code"
				? m.auth_code_title()
				: step === "details"
					? m.auth_details_title()
					: m.auth_terms_title();

	return (
		<AuthLayout
			footer={
				step === "email" ? (
					mode === "sign_in" ? (
						<p className="text-body text-fg-muted">
							{m.auth_new_here()}{" "}
							<Link
								to="/create"
								search={{ next, handoff }}
								className="font-medium text-fg underline decoration-line-strong underline-offset-4 hover:decoration-fg"
							>
								{m.auth_create_link()}
							</Link>
						</p>
					) : (
						<p className="text-body text-fg-muted">
							{m.auth_have_account()}{" "}
							<Link
								to="/sign-in"
								search={{ next, handoff }}
								className="font-medium text-fg underline decoration-line-strong underline-offset-4 hover:decoration-fg"
							>
								{m.auth_signin_link()}
							</Link>
						</p>
					)
				) : null
			}
		>
			<AnimatePresence mode="wait" initial={false}>
				<motion.div
					key={step}
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
							{step === "email" &&
								(mode === "sign_in" ? m.auth_signin_lead() : m.auth_create_lead())}
							{step === "code" && m.auth_code_lead({ email: hint })}
							{step === "details" && m.auth_details_lead({ email })}
							{step === "terms" && m.auth_terms_lead()}
						</p>
					</div>

					{step === "email" ? (
						<EmailStep
							mode={mode}
							passkey={support.available && mode === "sign_in"}
							passkeyBusy={passkeyBusy}
							passkeyError={passkeyError}
							onPasskey={signInWithPasskey}
							onSent={(e, challenge) => {
								setEmail(e);
								setHint(challenge.emailHint);
								setResendAt(Date.now() + challenge.resendAfter * 1000);
								go("code");
							}}
						/>
					) : null}
					{step === "code" ? (
						<CodeStep
							resendAt={resendAt}
							onResent={(secs) => setResendAt(Date.now() + secs * 1000)}
							onBack={() => go("email", -1)}
							onVerified={advance}
						/>
					) : null}
					{step === "details" ? <DetailsStep onDone={advance} /> : null}
					{step === "terms" ? <TermsStep onDone={advance} /> : null}
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
	onSent,
}: {
	mode: SignInIntent;
	passkey: boolean;
	passkeyBusy: boolean;
	passkeyError: string | null;
	onPasskey: () => void;
	onSent: (email: string, c: { emailHint: string; resendAfter: number }) => void;
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
			onSent(email, await api.auth.start({ email, intent: mode }));
		} catch (e) {
			setError("email", {
				message: isApiError(e) ? (e.fields.email ?? describeError(e)) : describeError(e),
			});
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
						// biome-ignore lint/a11y/noAutofocus: the page has exactly one field
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

function useCountdown(until: number) {
	const [left, setLeft] = useState(() => Math.max(0, Math.ceil((until - Date.now()) / 1000)));
	useEffect(() => {
		const tick = () => setLeft(Math.max(0, Math.ceil((until - Date.now()) / 1000)));
		tick();
		const id = window.setInterval(tick, 1000);
		return () => window.clearInterval(id);
	}, [until]);
	return left;
}

function CodeStep({
	resendAt,
	onResent,
	onBack,
	onVerified,
}: {
	resendAt: number;
	onResent: (seconds: number) => void;
	onBack: () => void;
	onVerified: (p: AuthProgress) => void;
}) {
	const api = useApi();
	const [code, setCode] = useState("");
	const [error, setError] = useState<string | null>(null);
	const [busy, setBusy] = useState(false);
	const [resending, setResending] = useState(false);
	const [sentNote, setSentNote] = useState(false);
	const left = useCountdown(resendAt);

	const verify = async (value: string) => {
		if (value.length < 6 || busy) return;
		setBusy(true);
		setError(null);
		try {
			onVerified(await api.auth.verifyCode(value));
		} catch (e) {
			setCode("");
			setError(isApiError(e) && e.fields.code ? e.fields.code : describeError(e));
		} finally {
			setBusy(false);
		}
	};

	const resend = async () => {
		setResending(true);
		setError(null);
		try {
			const c = await api.auth.resendCode();
			onResent(c.resendAfter);
			setSentNote(true);
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
					onChange={(v) => {
						setCode(v);
						setSentNote(false);
					}}
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
				{sentNote ? m.auth_resent() : ""}
			</p>
		</form>
	);
}

function DetailsStep({ onDone }: { onDone: (p: AuthProgress) => void }) {
	const api = useApi();
	const schema = useMemo(
		() =>
			z.object({
				handle: z
					.string()
					.trim()
					.regex(/^[A-Za-z][A-Za-z0-9_-]{2,15}$/, { error: m.auth_handle_rules() }),
				displayName: z.string().trim().max(40, { error: m.auth_display_name_long() }),
			}),
		[],
	);
	const {
		register,
		handleSubmit,
		setError,
		formState: { errors, isSubmitting },
	} = useForm<z.infer<typeof schema>>({
		resolver: zodResolver(schema),
		defaultValues: { handle: "", displayName: "" },
	});

	const submit = handleSubmit(async (values) => {
		try {
			onDone(await api.auth.submitDetails(values));
		} catch (e) {
			if (isApiError(e) && e.fields.handle) setError("handle", { message: e.fields.handle });
			else setError("handle", { message: describeError(e) });
		}
	});

	return (
		<form onSubmit={submit} noValidate className="flex flex-col gap-4">
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
					// biome-ignore lint/a11y/noAutofocus: first field of the step
					autoFocus
					{...register("handle")}
				/>
			</Field>
			<Field label={m.auth_display_name_label()} optional error={errors.displayName?.message}>
				<Input autoComplete="name" {...register("displayName")} />
			</Field>
			<Button type="submit" variant="primary" size="lg" loading={isSubmitting}>
				{m.common_continue()}
			</Button>
		</form>
	);
}

function TermsStep({ onDone }: { onDone: (p: AuthProgress) => void }) {
	const api = useApi();
	const qc = useQueryClient();
	const session = qc.getQueryData<{ terms?: { url: string | null; version: string } }>(qk.session);
	const terms = session?.terms ?? { url: null, version: "" };
	const [age, setAge] = useState(false);
	const [accepted, setAccepted] = useState(false);
	const [error, setError] = useState<string | null>(null);
	const [busy, setBusy] = useState(false);

	const submit = async () => {
		if (!age || !accepted) {
			setError(m.auth_terms_required());
			return;
		}
		setBusy(true);
		setError(null);
		try {
			onDone(await api.auth.acceptTerms());
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
			<fieldset className="flex flex-col gap-4">
				<legend className="sr-only">{m.auth_terms_title()}</legend>
				<label className="flex items-start gap-3 text-body">
					<Checkbox checked={accepted} onCheckedChange={setAccepted} className="mt-0.5" />
					<span>
						{terms.url ? (
							<>
								{m.auth_terms_accept_pre()}{" "}
								<a
									href={terms.url}
									target="_blank"
									rel="noopener noreferrer"
									className="font-medium underline decoration-line-strong underline-offset-4 hover:decoration-fg"
								>
									{m.auth_terms_link()}
									<span className="sr-only"> {m.common_new_tab()}</span>
								</a>
								{terms.version ? ` (${m.auth_terms_version({ version: terms.version })})` : null}.
							</>
						) : (
							m.auth_terms_accept_plain()
						)}
					</span>
				</label>
				<label className="flex items-start gap-3 text-body">
					<Checkbox checked={age} onCheckedChange={setAge} className="mt-0.5" />
					<span>
						{m.auth_age_label()}
						<span className="mt-0.5 block text-small text-fg-muted">{m.auth_age_hint()}</span>
					</span>
				</label>
			</fieldset>
			{error ? (
				<p role="alert" className="text-small text-danger-text">
					{error}
				</p>
			) : null}
			<Button type="submit" variant="primary" size="lg" loading={busy}>
				{m.auth_create_button()}
			</Button>
		</form>
	);
}
