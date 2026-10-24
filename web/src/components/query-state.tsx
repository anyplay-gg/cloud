import type { UseQueryResult } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { describeError } from "@/api/errors";
import { m } from "@/i18n";
import { ErrorState, LoadingRegion } from "@/ui/feedback";

/**
 * The four states of a read, in one place: loading (a skeleton shaped like the content),
 * failed (what went wrong and a retry), empty (what this is and how to fill it) and loaded.
 * Every list on every screen goes through this, so none of them can forget a state.
 */
export function QueryView<T>({
	query,
	loading,
	loadingLabel,
	isEmpty,
	empty,
	errorTitle,
	children,
}: {
	query: UseQueryResult<T>;
	loading: ReactNode;
	loadingLabel: string;
	isEmpty?: (data: T) => boolean;
	empty?: ReactNode;
	errorTitle?: string;
	children: (data: T) => ReactNode;
}) {
	if (query.isPending) return <LoadingRegion label={loadingLabel}>{loading}</LoadingRegion>;
	if (query.isError) {
		return (
			<ErrorState
				title={errorTitle ?? m.error_load_title()}
				description={describeError(query.error)}
				onRetry={() => void query.refetch()}
				retrying={query.isFetching}
			/>
		);
	}
	if (isEmpty?.(query.data) && empty) return <>{empty}</>;
	return <div className="fade-in">{children(query.data)}</div>;
}
