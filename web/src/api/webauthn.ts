// Passkey ceremonies through the browser's native WebAuthn JSON API
// (PublicKeyCredential.parse*OptionsFromJSON and credential.toJSON()). No library: every
// browser we support has it. Only the HTTP adapter uses this module; the mock skips ceremonies.
import { ApiError } from "./errors";

// Minimal shapes. The browser owns the full types; the server owns the option contents.
export type CreationOptionsJSON = Record<string, unknown>;
export type RequestOptionsJSON = Record<string, unknown>;
export type CredentialJSON = object;

interface JsonCapablePublicKeyCredential {
	parseCreationOptionsFromJSON?: (o: CreationOptionsJSON) => PublicKeyCredentialCreationOptions;
	parseRequestOptionsFromJSON?: (o: RequestOptionsJSON) => PublicKeyCredentialRequestOptions;
	isConditionalMediationAvailable?: () => Promise<boolean>;
}

const pkc = (): (JsonCapablePublicKeyCredential & typeof PublicKeyCredential) | null =>
	typeof window !== "undefined" && "PublicKeyCredential" in window
		? (window.PublicKeyCredential as JsonCapablePublicKeyCredential & typeof PublicKeyCredential)
		: null;

export async function passkeySupport(): Promise<{ available: boolean; conditional: boolean }> {
	const c = pkc();
	if (!c || !c.parseRequestOptionsFromJSON || !c.parseCreationOptionsFromJSON) {
		return { available: false, conditional: false };
	}
	let conditional = false;
	try {
		conditional = (await c.isConditionalMediationAvailable?.()) ?? false;
	} catch {
		conditional = false;
	}
	return { available: true, conditional };
}

function mapCeremonyError(e: unknown): ApiError {
	if (e instanceof ApiError) return e;
	if (e instanceof DOMException) {
		if (e.name === "NotAllowedError" || e.name === "AbortError") {
			return new ApiError("passkey_cancelled", "The passkey prompt was closed.");
		}
		if (e.name === "InvalidStateError") {
			return new ApiError("conflict", "This device already has a passkey for your account.");
		}
		if (e.name === "NotSupportedError" || e.name === "SecurityError") {
			return new ApiError("passkey_unsupported", "This browser can't use passkeys here.");
		}
	}
	return new ApiError("unknown", "The passkey prompt failed. Try again.");
}

export async function getAssertion(
	options: RequestOptionsJSON,
	{ conditional, signal }: { conditional: boolean; signal?: AbortSignal },
): Promise<CredentialJSON> {
	const c = pkc();
	if (!c?.parseRequestOptionsFromJSON)
		throw new ApiError("passkey_unsupported", "No passkey support.");
	try {
		const credential = (await navigator.credentials.get({
			publicKey: c.parseRequestOptionsFromJSON(options),
			mediation: conditional ? "conditional" : "optional",
			signal,
		})) as PublicKeyCredential | null;
		if (!credential) throw new ApiError("passkey_cancelled", "No passkey was chosen.");
		return credential.toJSON();
	} catch (e) {
		throw mapCeremonyError(e);
	}
}

export async function createCredential(options: CreationOptionsJSON): Promise<CredentialJSON> {
	const c = pkc();
	if (!c?.parseCreationOptionsFromJSON)
		throw new ApiError("passkey_unsupported", "No passkey support.");
	try {
		const credential = (await navigator.credentials.create({
			publicKey: c.parseCreationOptionsFromJSON(options),
		})) as PublicKeyCredential | null;
		if (!credential) throw new ApiError("passkey_cancelled", "No passkey was created.");
		return credential.toJSON();
	} catch (e) {
		throw mapCeremonyError(e);
	}
}
