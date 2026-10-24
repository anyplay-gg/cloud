import { type QueryKey, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { isStepUpCancelled, useStepUp } from "@/features/step-up";
import { toast } from "@/ui/toast";
import { useApi } from "./context";
import { describeError, isApiError } from "./errors";
import type { PortalApi } from "./portal-api";

export const qk = {
	session: ["session"] as const,
	account: ["account"] as const,
	privacy: ["privacy"] as const,
	profile: ["profile"] as const,
	friends: ["friends"] as const,
	trophies: ["trophies"] as const,
	user: (handle: string) => ["user", handle.toLowerCase()] as const,
	ban: ["ban"] as const,
};

// A failed read is a state the screen shows with a retry; do not hammer the cloud behind it.
const noRetry = { retry: false } as const;

export const useSession = () => {
	const api = useApi();
	return useQuery({
		queryKey: qk.session,
		queryFn: () => api.session(),
		staleTime: 60_000,
		...noRetry,
	});
};

/** `GET /account`: the passkeys, launchers, browsers, deletion and pending address, in one read. */
export const useAccountView = () => {
	const api = useApi();
	return useQuery({ queryKey: qk.account, queryFn: () => api.account.get(), ...noRetry });
};
export const usePasskeys = () => {
	const api = useApi();
	return useQuery({
		queryKey: qk.account,
		queryFn: () => api.account.get(),
		select: (a) => a.passkeys,
		...noRetry,
	});
};
export const useDevices = () => {
	const api = useApi();
	return useQuery({
		queryKey: qk.account,
		queryFn: () => api.account.get(),
		select: (a) => ({ devices: a.devices, sessions: a.sessions }),
		...noRetry,
	});
};
export const usePrivacy = () => {
	const api = useApi();
	return useQuery({ queryKey: qk.privacy, queryFn: () => api.account.privacy(), ...noRetry });
};
export const useOwnProfile = () => {
	const api = useApi();
	return useQuery({ queryKey: qk.profile, queryFn: () => api.account.profile(), ...noRetry });
};
export const useFriends = () => {
	const api = useApi();
	return useQuery({ queryKey: qk.friends, queryFn: () => api.friends.lists(), ...noRetry });
};
export const useTrophies = () => {
	const api = useApi();
	return useQuery({ queryKey: qk.trophies, queryFn: () => api.trophies.mine(), ...noRetry });
};
export const usePublicUser = (handle: string) => {
	const api = useApi();
	return useQuery({
		queryKey: qk.user(handle),
		queryFn: () => api.users.get(handle),
		...noRetry,
	});
};
export const useBan = () => {
	const api = useApi();
	return useQuery({ queryKey: qk.ban, queryFn: () => api.auth.ban(), ...noRetry });
};

interface ActionOptions<V, R> {
	/** Toast on success, "Noun past-participle": "Passkey removed". */
	success?: string | ((result: R, vars: V) => string | undefined);
	/** Query keys to refetch after the write. */
	invalidate?: readonly QueryKey[];
	/** Show a toast for a failure. Turn off when the form shows the error under its field. */
	toastErrors?: boolean;
	onSuccess?: (result: R, vars: V) => void;
}

/**
 * A write with the portal's habits built in: it goes through the step-up dialog when the cloud
 * asks for a recent sign-in, refreshes what changed, and says what happened in a toast. A person
 * who closes the step-up dialog gets silence, not an error.
 */
export function useAction<V, R>(
	write: (vars: V, api: PortalApi) => Promise<R>,
	options: ActionOptions<V, R> = {},
) {
	const api = useApi();
	const stepUp = useStepUp();
	const qc = useQueryClient();
	const { toastErrors = true } = options;
	return useMutation<R, unknown, V>({
		mutationFn: (vars) => stepUp.run(() => write(vars, api)),
		onSuccess: (result, vars) => {
			for (const key of options.invalidate ?? []) void qc.invalidateQueries({ queryKey: key });
			const text =
				typeof options.success === "function" ? options.success(result, vars) : options.success;
			if (text) toast.success(text);
			options.onSuccess?.(result, vars);
		},
		onError: (e) => {
			if (isStepUpCancelled(e)) return;
			if (isApiError(e) && e.code === "unauthorized") {
				void qc.invalidateQueries({ queryKey: qk.session });
				return;
			}
			if (toastErrors && !(isApiError(e) && Object.keys(e.fields).length > 0)) {
				toast.error(describeError(e));
			}
		},
	});
}
