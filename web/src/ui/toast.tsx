import { Toast } from "@base-ui/react/toast";
import { CircleAlert, CircleCheck, X } from "lucide-react";
import type { ReactNode } from "react";

/**
 * Sonner was the first choice, but it injects a <style> element at runtime and the CSP is
 * `style-src 'self'`. Base UI's toast is already a dependency and stacks the same way.
 */
const manager = Toast.createToastManager();

/** Call from anywhere, including outside React. Text is "Noun past-participle", no "successfully". */
export const toast = {
	success: (title: string) => manager.add({ title, type: "success", timeout: 4000 }),
	// errors and anything with an action stay until dismissed
	error: (title: string, description?: string) =>
		manager.add({ title, description, type: "error", timeout: 0 }),
	dismiss: (id?: string) => manager.close(id),
};

export function ToastProvider({ children }: { children: ReactNode }) {
	return (
		<Toast.Provider toastManager={manager} limit={3}>
			{children}
			<Toast.Portal>
				<Toast.Viewport
					className="fixed inset-x-3 bottom-3 z-[70] mx-auto w-auto sm:inset-x-auto sm:end-5 sm:bottom-5 sm:mx-0 sm:w-[22rem]"
					style={{ paddingBottom: "env(safe-area-inset-bottom)" }}
				>
					<Toasts />
				</Toast.Viewport>
			</Toast.Portal>
		</Toast.Provider>
	);
}

function Toasts() {
	const { toasts } = Toast.useToastManager();
	return toasts.map((t) => (
		<Toast.Root
			key={t.id}
			toast={t}
			className="toast absolute inset-x-0 bottom-0 flex rounded-lg bg-surface p-3.5 text-fg shadow-pop select-none"
		>
			<Toast.Content className="flex w-full items-start gap-3">
				{t.type === "error" ? (
					<CircleAlert aria-hidden className="mt-0.5 size-4 shrink-0 text-danger-text" />
				) : (
					<CircleCheck aria-hidden className="mt-0.5 size-4 shrink-0 text-success-text" />
				)}
				<div className="min-w-0 flex-1">
					<Toast.Title className="text-body font-medium" />
					<Toast.Description className="mt-0.5 text-small text-fg-muted" />
				</div>
				<Toast.Close
					aria-label="Dismiss"
					className="press -m-1.5 flex size-8 shrink-0 items-center justify-center rounded-md text-fg-muted pointer-hover:hover:bg-hover pointer-hover:hover:text-fg coarse:size-11"
				>
					<X aria-hidden className="size-4" />
				</Toast.Close>
			</Toast.Content>
		</Toast.Root>
	));
}
