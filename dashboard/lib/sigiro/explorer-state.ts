/**
 * The explorer's results as one union, derived from React Query's infinite
 * query. Every consumer (the table-level banner, the empty slot, the
 * load-more control) reads this instead of combining `isFetching`,
 * `isLoading`, `error` and `hasNextPage` on its own, so a refresh can no
 * longer flip the load-more button and a failed next page can no longer
 * raise the table-level banner.
 *
 * React Query already is the state machine for the request; this names its
 * states. A second machine (XState) would only duplicate it.
 */

/** What the control under the rows offers. */
export type MoreState =
  /** Another page exists: offer "Load more". */
  | "idle"
  /** That page is in flight. */
  | "loading"
  /** That page failed: retry inline, keep the rows and the table quiet. */
  | "failed"
  /** Nothing further in this query. */
  | "end";

export type ExplorerView =
  /** First load, nothing to show yet. */
  | { kind: "loading" }
  /** First load failed; `retrying` while the retry is in flight. */
  | { kind: "failed"; error: Error; retrying: boolean }
  /** Loaded, nothing matched; `refreshing` while a refresh/live tick runs. */
  | { kind: "empty"; refreshing: boolean }
  /**
   * Rows on screen. `refreshing` is the header refresh/live tick and never
   * changes `more`; `refreshError` is a failed refresh shown above the
   * still-visible rows; `moreError` belongs to the load-more control only.
   */
  | {
      kind: "rows";
      more: MoreState;
      refreshing: boolean;
      refreshError: Error | null;
      moreError: Error | null;
    };

/** The slice of `useInfiniteQuery`'s result the view depends on. */
export type ExplorerSnapshot = {
  rows: number;
  status: "pending" | "error" | "success";
  error: Error | null;
  isFetching: boolean;
  isFetchingNextPage: boolean;
  isFetchNextPageError: boolean;
  /** `canLoadMore(...)`: another page would return rows. */
  hasMore: boolean;
};

export function explorerView(snapshot: ExplorerSnapshot): ExplorerView {
  const error = snapshot.status === "error" ? snapshot.error : null;
  if (snapshot.rows === 0) {
    if (snapshot.status === "pending") return { kind: "loading" };
    if (error) return { kind: "failed", error, retrying: snapshot.isFetching };
    return { kind: "empty", refreshing: snapshot.isFetching };
  }
  const pageError = snapshot.isFetchNextPageError ? error : null;
  const more: MoreState = snapshot.isFetchingNextPage
    ? "loading"
    : pageError
      ? "failed"
      : snapshot.hasMore
        ? "idle"
        : "end";
  return {
    kind: "rows",
    more,
    refreshing: snapshot.isFetching && !snapshot.isFetchingNextPage,
    refreshError: pageError ? null : error,
    moreError: pageError,
  };
}

/**
 * Keyset cursor `(timestamp, key)` for rows ordered newest first. A bare
 * timestamp cursor skipped every row that shared the page's last timestamp
 * (to the millisecond) but did not fit on the page.
 */
export type Keyset = { micros: number; key: string };

export const encodeKeyset = ({ micros, key }: Keyset) => `${micros}~${key}`;

export function decodeKeyset(value: unknown): Keyset | null {
  if (typeof value !== "string") return null;
  const split = value.indexOf("~");
  if (split <= 0) return null;
  const micros = Number(value.slice(0, split));
  if (!Number.isSafeInteger(micros)) return null;
  return { micros, key: value.slice(split + 1) };
}
