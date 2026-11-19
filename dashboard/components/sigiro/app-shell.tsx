"use client";

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { Menu, MenuTrigger, MenuContent, MenuItem } from "@ui-sdk/react/menu";
import { OverlayHost } from "@ui-sdk/react/overlays";
import Link from "next/link";
import { usePathname } from "next/navigation";
import { NuqsAdapter } from "nuqs/adapters/next/app";
import * as React from "react";

import { AgentBridge } from "@/components/sigiro/agent-bridge";
import { AlertDeliveryNotice } from "@/components/sigiro/alert-delivery-notice";
import {
  AuthGate,
  useLocalMode,
  useWorkspace,
} from "@/components/sigiro/auth-gate";
import { SiteHeader } from "@/components/site-header";
import { ControlsProvider, useControls } from "@/lib/sigiro/controls";
import { auth } from "@/lib/sigiro/query";

const pages = [
  ["/", "Overview"],
  ["/traces/", "Traces"],
  ["/logs/", "Logs"],
  ["/profiles/", "Profiles"],
  ["/sql/", "SQL"],
] as const;

function Navigation() {
  const pathname = usePathname();
  const workspace = useWorkspace();
  const local = useLocalMode();
  const [authError, setAuthError] = React.useState<string>();
  return (
    <div className="flex flex-wrap items-center gap-x-8 gap-y-2 border-b px-4 py-3 lg:px-6">
      <Link href="/" className="font-semibold tracking-tight no-underline">
        sigiro
      </Link>
      <nav
        aria-label="Main navigation"
        className="order-last flex w-full min-w-0 gap-1 overflow-x-auto text-sm sm:order-none sm:w-auto"
      >
        {pages.map(([href, label]) => (
          <Link
            key={href}
            href={href}
            aria-current={
              pathname.replace(/\/$/, "") === href.replace(/\/$/, "")
                ? "page"
                : undefined
            }
            className="text-muted-foreground hover:bg-muted hover:text-foreground aria-[current=page]:bg-primary aria-[current=page]:text-primary-foreground rounded-md px-3 py-1.5 no-underline"
          >
            {label}
          </Link>
        ))}
      </nav>
      <Menu>
        <MenuTrigger
          aria-label={`${workspace ? `${workspace} ` : ""}Workspace menu`}
          className="ml-auto"
        >
          {workspace ?? "Workspace"}
        </MenuTrigger>
        <MenuContent align="end" sideOffset={8}>
          <MenuItem value="docs" render={<a href="/docs" />}>
            Documentation
          </MenuItem>
          {!local && (
            <MenuItem
              value="switch"
              onSelect={() => {
                auth.forget();
                void auth.beginAuthorization().catch((error) => {
                  if (!String(error).includes("sign-in flow has started"))
                    setAuthError(
                      error instanceof Error ? error.message : String(error)
                    );
                });
              }}
            >
              Switch workspace
            </MenuItem>
          )}
          {!local && (
            <MenuItem
              value="sign-out"
              onSelect={() => {
                auth.forget();
                location.reload();
              }}
            >
              Sign out
            </MenuItem>
          )}
        </MenuContent>
      </Menu>
      {authError && (
        <p role="alert" className="text-destructive text-sm">
          {authError}
        </p>
      )}
    </div>
  );
}

function WindowGuard({ children }: { children: React.ReactNode }) {
  const { windowError } = useControls();
  const pathname = usePathname();
  if (windowError && pathname.replace(/\/$/, "") !== "/sql")
    return (
      <div role="alert" className="space-y-2 p-6 text-sm">
        <h2 className="font-semibold">Invalid time range</h2>
        <p>{windowError}. Choose a valid range in the header to continue.</p>
      </div>
    );
  return children;
}

export function AppShell({ children }: { children: React.ReactNode }) {
  const [queryClient] = React.useState(() => new QueryClient());
  return (
    <QueryClientProvider client={queryClient}>
      <NuqsAdapter>
        <React.Suspense>
          <AuthGate>
            <ControlsProvider>
              <div className="flex h-svh min-w-0 flex-col">
                <header className="shrink-0">
                  <a
                    href="#main-content"
                    className="focus:bg-background sr-only focus:not-sr-only focus:absolute focus:z-50 focus:p-3"
                  >
                    Skip to content
                  </a>
                  <Navigation />
                  <SiteHeader />
                </header>
                <AlertDeliveryNotice />
                <main
                  id="main-content"
                  tabIndex={-1}
                  className="@container/main flex min-h-0 flex-1 flex-col overflow-y-auto outline-none"
                >
                  <WindowGuard>{children}</WindowGuard>
                </main>
              </div>
              <OverlayHost />
              <AgentBridge />
            </ControlsProvider>
          </AuthGate>
        </React.Suspense>
      </NuqsAdapter>
    </QueryClientProvider>
  );
}
