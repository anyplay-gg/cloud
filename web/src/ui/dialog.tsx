import { AlertDialog as BaseAlertDialog } from "@base-ui/react/alert-dialog";
import { Dialog as BaseDialog } from "@base-ui/react/dialog";
import { X } from "lucide-react";
import { type ReactNode, type RefObject, useRef } from "react";
import { cn } from "@/lib/cn";
import { Button, type ButtonProps } from "./button";

const backdropClass = "backdrop fixed inset-0 z-40 bg-scrim";
const viewportClass =
	"fixed inset-0 z-50 flex items-end justify-center overflow-y-auto sm:items-center sm:p-6";
const popupClass =
	"dialog-pop relative flex w-full flex-col bg-surface text-fg shadow-dialog outline-none " +
	"max-h-[min(100dvh-1rem,44rem)] rounded-t-2xl sm:max-w-md sm:rounded-2xl pb-[env(safe-area-inset-bottom)]";

export const Dialog = BaseDialog.Root;
export const DialogTrigger = BaseDialog.Trigger;
export const DialogClose = BaseDialog.Close;

export interface DialogContentProps {
	title: ReactNode;
	description?: ReactNode;
	children?: ReactNode;
	footer?: ReactNode;
	className?: string;
	/** Hide the corner close button when the footer already has a way out. */
	hideClose?: boolean;
	initialFocus?: RefObject<HTMLElement | null>;
}

/** A modal. Centred on wide screens, a bottom sheet on phones. Escape and the scrim close it. */
export function DialogContent({
	title,
	description,
	children,
	footer,
	className,
	hideClose,
	initialFocus,
}: DialogContentProps) {
	return (
		<BaseDialog.Portal>
			<BaseDialog.Backdrop className={backdropClass} />
			<BaseDialog.Viewport className={viewportClass}>
				<BaseDialog.Popup initialFocus={initialFocus} className={cn(popupClass, className)}>
					<div className="flex flex-col gap-1 p-5 pe-14 pb-0 sm:p-6 sm:pe-14 sm:pb-0">
						<BaseDialog.Title className="text-lead font-semibold text-fg">{title}</BaseDialog.Title>
						{description ? (
							<BaseDialog.Description className="text-body text-fg-muted">
								{description}
							</BaseDialog.Description>
						) : null}
					</div>
					{hideClose ? null : (
						<BaseDialog.Close
							aria-label="Close"
							className="press absolute end-3 top-3 flex size-8 items-center justify-center rounded-md text-fg-muted pointer-hover:hover:bg-hover pointer-hover:hover:text-fg coarse:size-11"
						>
							<X aria-hidden className="size-4" />
						</BaseDialog.Close>
					)}
					{children ? (
						<div className="min-h-0 overflow-y-auto p-5 sm:p-6">{children}</div>
					) : (
						<div className="h-5" />
					)}
					{footer ? (
						<div className="flex flex-col-reverse gap-3 px-5 pb-5 sm:flex-row sm:justify-end sm:px-6 sm:pb-6">
							{footer}
						</div>
					) : null}
				</BaseDialog.Popup>
			</BaseDialog.Viewport>
		</BaseDialog.Portal>
	);
}

export interface ConfirmDialogProps {
	open: boolean;
	onOpenChange: (open: boolean) => void;
	title: ReactNode;
	description?: ReactNode;
	/** Verb + noun: "Delete account", "Remove passkey". */
	confirmLabel: string;
	cancelLabel?: string;
	tone?: "danger" | "neutral";
	pending?: boolean;
	onConfirm: () => void;
	/** Extra content between the description and the buttons, for example a type-to-confirm field. */
	children?: ReactNode;
	confirmDisabled?: boolean;
}

/**
 * For anything destructive. Focus lands on Cancel, so Enter never deletes by accident; the
 * confirm button repeats the consequence in its own label.
 */
export function ConfirmDialog({
	open,
	onOpenChange,
	title,
	description,
	confirmLabel,
	cancelLabel = "Cancel",
	tone = "danger",
	pending,
	onConfirm,
	children,
	confirmDisabled,
}: ConfirmDialogProps) {
	const cancelRef = useRef<HTMLButtonElement>(null);
	return (
		<BaseAlertDialog.Root
			open={open}
			onOpenChange={(o) => (!pending ? onOpenChange(o) : undefined)}
		>
			<BaseAlertDialog.Portal>
				<BaseAlertDialog.Backdrop className={backdropClass} />
				<BaseAlertDialog.Viewport className={viewportClass}>
					<BaseAlertDialog.Popup initialFocus={cancelRef} className={cn(popupClass)}>
						<div className="flex flex-col gap-1.5 p-5 pb-0 sm:p-6 sm:pb-0">
							<BaseAlertDialog.Title className="text-lead font-semibold text-fg">
								{title}
							</BaseAlertDialog.Title>
							{description ? (
								<BaseAlertDialog.Description className="text-body text-fg-muted">
									{description}
								</BaseAlertDialog.Description>
							) : null}
						</div>
						{children ? <div className="p-5 pb-0 sm:p-6 sm:pb-0">{children}</div> : null}
						<div className="flex flex-col-reverse gap-3 p-5 sm:flex-row sm:justify-end sm:p-6">
							<BaseAlertDialog.Close
								ref={cancelRef}
								disabled={pending}
								render={<Button variant="secondary" size="md" />}
							>
								{cancelLabel}
							</BaseAlertDialog.Close>
							<Button
								variant={tone === "danger" ? "danger" : "primary"}
								loading={pending}
								disabled={confirmDisabled}
								onClick={onConfirm}
							>
								{confirmLabel}
							</Button>
						</div>
					</BaseAlertDialog.Popup>
				</BaseAlertDialog.Viewport>
			</BaseAlertDialog.Portal>
		</BaseAlertDialog.Root>
	);
}

export type { ButtonProps };
