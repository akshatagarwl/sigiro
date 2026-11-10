"use client";

import type { ReactNode } from "react";

import { Button } from "./primitives";

import styles from "./observability.module.css";

/** Presentation only: the host owns request identity, data, cancellation and retry. */
export function RequestState({
  kind,
  title,
  children,
  onRetry,
  retrying = false,
}: {
  kind: "idle" | "loading" | "updating" | "empty" | "error";
  title: string;
  children?: ReactNode;
  onRetry?: () => void | Promise<unknown>;
  retrying?: boolean;
}) {
  return (
    <section
      className={styles.requestState}
      data-request-state={kind}
      role={kind === "error" ? "alert" : "status"}
      aria-busy={kind === "loading" || kind === "updating" || retrying}
    >
      <div>
        <h2>{title}</h2>
        {children && (
          <div className={styles.requestDescription}>{children}</div>
        )}
      </div>
      {onRetry && (
        <Button disabled={retrying} onClick={() => void onRetry()}>
          {retrying ? "Retrying…" : "Retry"}
        </Button>
      )}
    </section>
  );
}
