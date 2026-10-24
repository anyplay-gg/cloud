import { KeyRound, Mail } from "lucide-react";
import {
	createContext,
	type ReactNode,
	useCallback,
	useContext,
	useEffect,
	useMemo,
	useRef,
	useState,
} from "react";
import { useApi } from "@/api/context";
import { describeError, isApiError, isStepUp } from "@/api/errors";
import type { StepUpMethod } from "@/api/types";
import { m } from "@/i18n";
import { Button } from "@/ui/button";
import { Dialog, DialogContent } from "@/ui/dialog";
import { Field } from "@/ui/field";
import { OtpInput } from "@/ui/otp-input";

/** Thrown out of `run` when the person closes the dialog without confirming. */
export class StepUpCancelled extends Error {
	constructor() {
		super("Step-up cancelled");
		this.name = "StepUpCancelled";
	}
}
export const isStepUpCancelled = (e: unknown): e is StepUpCancelled => e instanceof StepUpCancelled;

interface StepUp {
	/**
	 * Runs a write. When the cloud answers `step_up_required`, shows the confirm dialog, then runs
	 * the write once more. Rejects with `StepUpCancelled` if the person backs out.
	 */
	run<T>(action: () => Promise<T>): Promise<T>;
}

const Ctx = createContext<StepUp | null>(null);

interface Pending {
	methods: StepUpMethod[];
	resolve: () => void;
	reject: (e: unknown) => void;
}

export function StepUpProvider({ children }: { children: ReactNode }) {
	const [pending, setPending] = useState<Pending | null>(null);

	const run = useCallback(async <T,>(action: () => Promise<T>): Promise<T> => {
		try {
			return await action();
		} catch (e) {
			if (!isStepUp(e)) throw e;
			const methods =
				isApiError(e) && e.methods.length > 0 ? e.methods : (["email_code"] as StepUpMethod[]);
			await new Promise<void>((resolve, reject) => setPending({ methods, resolve, reject }));
			return action();
		}
	}, []);

	const value = useMemo(() => ({ run }), [run]);
	const close = (ok: boolean) => {
		if (!pending) return;
		if (ok) pending.resolve();
		else pending.reject(new StepUpCancelled());
		setPending(null);
	};

	return (
		<Ctx.Provider value={value}>
			{children}
			<Dialog open={pending !== null} onOpenChange={(o) => !o && close(false)}>
				{pending ? <StepUpBody methods={pending.methods} onDone={() => close(true)} /> : null}
			</Dialog>
		</Ctx.Provider>
	);
}

export function useStepUp(): StepUp {
	const v = useContext(Ctx);
	if (!v) throw new Error("useStepUp needs <StepUpProvider>");
	return v;
}

function StepUpBody({ methods, onDone }: { methods: StepUpMethod[]; onDone: () => void }) {
	const api = useApi();
	const canPasskey = methods.includes("passkey");
	const canEmail = methods.includes("email_code");
	const [mode, setMode] = useState<"choose" | "code">(canPasskey ? "choose" : "code");
	const [hint, setHint] = useState<string | null>(null);
	const [code, setCode] = useState("");
	const [busy, setBusy] = useState<"passkey" | "send" | "verify" | null>(null);
	const [error, setError] = useState<string | null>(null);

	const sendCode = useCallback(async () => {
		setBusy("send");
		setError(null);
		try {
			setHint(await api.stepUp.sendEmailCode());
			setMode("code");
		} catch (e) {
			setError(describeError(e));
		} finally {
			setBusy(null);
		}
	}, [api]);

	// A person with no passkey goes straight to the code step, so send it once on open.
	const started = useRef(false);
	useEffect(() => {
		if (canPasskey || !canEmail || started.current) return;
		started.current = true;
		void sendCode();
	}, [canPasskey, canEmail, sendCode]);

	const confirmWithPasskey = async () => {
		setBusy("passkey");
		setError(null);
		try {
			await api.stepUp.withPasskey();
			onDone();
		} catch (e) {
			if (isApiError(e) && e.code === "passkey_cancelled") setError(m.stepup_passkey_cancelled());
			else setError(describeError(e));
		} finally {
			setBusy(null);
		}
	};

	const verify = async (value: string) => {
		setBusy("verify");
		setError(null);
		try {
			await api.stepUp.verifyEmailCode(value);
			onDone();
		} catch (e) {
			setCode("");
			setError(isApiError(e) && e.fields.code ? e.fields.code : describeError(e));
		} finally {
			setBusy(null);
		}
	};

	return (
		<DialogContent title={m.stepup_title()} description={m.stepup_description()}>
			{mode === "choose" ? (
				<div className="flex flex-col gap-3">
					<Button
						variant="primary"
						size="lg"
						onClick={confirmWithPasskey}
						loading={busy === "passkey"}
					>
						<KeyRound aria-hidden />
						{m.stepup_use_passkey()}
					</Button>
					{canEmail ? (
						<Button variant="secondary" size="lg" onClick={sendCode} loading={busy === "send"}>
							<Mail aria-hidden />
							{m.stepup_email_me()}
						</Button>
					) : null}
					{error ? (
						<p role="alert" className="text-small text-danger-text">
							{error}
						</p>
					) : null}
				</div>
			) : (
				<form
					className="flex flex-col gap-4"
					onSubmit={(e) => {
						e.preventDefault();
						if (code.length === 6) void verify(code);
					}}
				>
					<Field
						label={hint ? m.stepup_code_label({ email: hint }) : m.stepup_code_label_plain()}
						error={error}
					>
						<OtpInput
							value={code}
							onChange={setCode}
							onComplete={(v) => void verify(v)}
							invalid={Boolean(error)}
							disabled={busy === "verify"}
							autoFocus
						/>
					</Field>
					<div className="flex flex-col-reverse gap-3 sm:flex-row sm:items-center sm:justify-between">
						{canPasskey ? (
							<Button variant="ghost" type="button" onClick={() => setMode("choose")}>
								{m.stepup_other_way()}
							</Button>
						) : (
							<Button variant="ghost" type="button" onClick={sendCode} loading={busy === "send"}>
								{m.auth_resend()}
							</Button>
						)}
						<Button
							variant="primary"
							type="submit"
							loading={busy === "verify"}
							disabled={code.length < 6}
						>
							{m.common_confirm()}
						</Button>
					</div>
				</form>
			)}
		</DialogContent>
	);
}
