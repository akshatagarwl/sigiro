"use client";

import * as React from "react";

import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { auth } from "@/lib/sigiro/query";

type Phase =
  | { kind: "starting" }
  | { kind: "signed-out"; reason?: string }
  | { kind: "signed-in"; workspace?: string };

const WorkspaceContext = React.createContext<string | undefined>(undefined);
export const useWorkspace = () => React.useContext(WorkspaceContext);

/** The `tid` claim names the workspace the query API scopes to. Display only. */
const tenantOf = (token: string): string | undefined => {
  try {
    const payload = JSON.parse(
      atob(
        (token.split(".")[1] ?? "").replaceAll("-", "+").replaceAll("_", "/")
      )
    ) as { tid?: unknown };
    return typeof payload.tid === "string" ? payload.tid : undefined;
  } catch {
    return undefined;
  }
};

/** The workspace's display name from the signed-in session, when there is one. */
const nameOf = async (id: string): Promise<string | undefined> => {
  try {
    const response = await fetch("/api/auth/organization/list", {
      credentials: "include",
    });
    if (!response.ok) return undefined;
    const orgs = (await response.json()) as { id: string; name: string }[];
    return orgs.find((org) => org.id === id)?.name;
  } catch {
    return undefined;
  }
};

export function AuthGate({ children }: { children: React.ReactNode }) {
  const [phase, setPhase] = React.useState<Phase>({ kind: "starting" });

  React.useEffect(() => {
    const start = async () => {
      try {
        await auth.handleCallback();
      } catch (error) {
        setPhase({
          kind: "signed-out",
          reason: `Sign-in did not complete: ${error instanceof Error ? error.message : String(error)}`,
        });
        return;
      }
      const token = auth.currentToken();
      if (!token) {
        setPhase({ kind: "signed-out" });
        return;
      }
      const tid = tenantOf(token);
      // The id itself (YEx3T2PJ…) means nothing to a reader, so only a
      // resolved name is ever shown.
      setPhase({ kind: "signed-in" });
      if (tid) {
        const name = await nameOf(tid);
        if (name) setPhase({ kind: "signed-in", workspace: name });
      }
    };
    void start();
  }, []);

  if (phase.kind === "starting")
    return (
      <div className="text-muted-foreground p-6 text-sm" data-state="starting">
        Checking sign-in…
      </div>
    );

  if (phase.kind === "signed-out")
    return (
      <div className="flex min-h-svh items-center justify-center p-6">
        <Card className="w-full max-w-sm" data-state="signed-out">
          <CardHeader>
            <CardTitle>Sign in to Sigiro</CardTitle>
            <CardDescription>
              See your workspace&apos;s traces, logs and services. Signing in
              gives this tab read-only access for 15 minutes.
            </CardDescription>
          </CardHeader>
          <CardContent className="grid gap-4">
            {phase.reason && (
              <p className="text-destructive text-sm" role="alert">
                {phase.reason}
              </p>
            )}
            <Button
              data-action="sign-in"
              onClick={() =>
                void auth.beginAuthorization().catch(() => undefined)
              }
            >
              Sign in
            </Button>
          </CardContent>
        </Card>
      </div>
    );

  return (
    <WorkspaceContext.Provider value={phase.workspace}>
      {children}
    </WorkspaceContext.Provider>
  );
}
