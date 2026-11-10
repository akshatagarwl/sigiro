"use client";

import { IconAlertTriangle } from "@tabler/icons-react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { Button } from "@/components/ui/button";
import { auth, localToken } from "@/lib/sigiro/query";

/** `GET /v1/alerts` → `delivery`: the project's alert webhook outbox. */
export type DeliverySummary = {
  queued: number;
  dead: number;
  oldest_queued_at?: string;
  last_dead_error?: string;
};

const KEY = ["sigiro", "alert-delivery"] as const;

const bearer = () => auth.currentToken() ?? localToken();

/**
 * Reads the outbox summary. Servers without alerting (the hosted cell) or
 * tokens without `alerts:read` answer non-2xx; that hides the notice rather
 * than showing an error, because there is nothing the reader can act on.
 */
async function fetchDelivery(
  signal: AbortSignal
): Promise<DeliverySummary | null> {
  const token = bearer();
  if (!token) return null;
  const response = await fetch("/v1/alerts?status=firing", {
    credentials: "omit",
    headers: { authorization: `Bearer ${token}`, accept: "application/json" },
    signal,
  });
  if (!response.ok) return null;
  const body = (await response.json()) as { delivery?: DeliverySummary };
  return body.delivery ?? null;
}

async function replayDead(): Promise<number> {
  const token = bearer();
  if (!token) throw new Error("Not signed in.");
  const response = await fetch("/v1/alerts/replay", {
    method: "POST",
    credentials: "omit",
    headers: { authorization: `Bearer ${token}`, accept: "application/json" },
  });
  if (!response.ok) throw new Error(`Replay failed (${response.status}).`);
  return ((await response.json()) as { replayed: number }).replayed;
}

/** One line under the header when alert webhooks are dead, with a replay button. */
export function AlertDeliveryNotice() {
  const client = useQueryClient();
  const delivery = useQuery({
    queryKey: KEY,
    queryFn: ({ signal }) => fetchDelivery(signal),
    refetchInterval: 60_000,
    retry: false,
  });
  const replay = useMutation({
    mutationFn: replayDead,
    onSettled: () => client.invalidateQueries({ queryKey: KEY }),
  });
  const dead = delivery.data?.dead ?? 0;
  if (dead === 0) return null;
  const reason = delivery.data?.last_dead_error;
  return (
    <div
      role="status"
      className="border-warning/40 bg-warning/10 flex flex-wrap items-center gap-2 border-b px-4 py-2 text-sm lg:px-6"
    >
      <IconAlertTriangle className="text-warning size-4 shrink-0" aria-hidden />
      <span className="min-w-0 flex-1">
        {dead === 1 ? "1 alert webhook" : `${dead} alert webhooks`} stopped
        retrying
        {reason ? ` (last error ${reason})` : ""}.
        {replay.isError ? ` ${replay.error.message}` : ""}
      </span>
      <Button
        size="sm"
        variant="outline"
        disabled={replay.isPending}
        onClick={() => replay.mutate()}
      >
        {replay.isPending ? "Replaying…" : "Replay"}
      </Button>
    </div>
  );
}
