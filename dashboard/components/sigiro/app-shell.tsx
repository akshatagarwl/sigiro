"use client";

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { NuqsAdapter } from "nuqs/adapters/next/app";
import * as React from "react";

import { AppSidebar } from "@/components/app-sidebar";
import { AuthGate } from "@/components/sigiro/auth-gate";
import { SiteHeader } from "@/components/site-header";
import { SidebarInset, SidebarProvider } from "@/components/ui/sidebar";
import { Toaster } from "@/components/ui/sonner";
import { TooltipProvider } from "@/components/ui/tooltip";
import { ControlsProvider } from "@/lib/sigiro/controls";

/** dashboard-01's sidebar + inset layout, behind sign-in, with the providers the blocks need. */
export function AppShell({ children }: { children: React.ReactNode }) {
  const [queryClient] = React.useState(() => new QueryClient());
  return (
    <QueryClientProvider client={queryClient}>
      <NuqsAdapter>
        <TooltipProvider>
          <React.Suspense>
            <AuthGate>
              <ControlsProvider>
                <SidebarProvider
                  style={
                    {
                      "--sidebar-width": "calc(var(--spacing) * 60)",
                      "--header-height": "calc(var(--spacing) * 12)",
                    } as React.CSSProperties
                  }
                >
                  <AppSidebar variant="inset" />
                  <SidebarInset className="min-w-0">
                    <SiteHeader />
                    <div className="@container/main flex min-h-0 flex-1 flex-col">
                      {children}
                    </div>
                  </SidebarInset>
                </SidebarProvider>
              </ControlsProvider>
            </AuthGate>
          </React.Suspense>
          <Toaster />
        </TooltipProvider>
      </NuqsAdapter>
    </QueryClientProvider>
  );
}
