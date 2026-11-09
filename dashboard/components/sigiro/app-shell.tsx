"use client";

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { usePathname } from "next/navigation";
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
  const isLogs = usePathname().replace(/\/$/, "") === "/logs";
  const [navigationOpen, setNavigationOpen] = React.useState(!isLogs);
  React.useEffect(() => setNavigationOpen(!isLogs), [isLogs]);
  return (
    <QueryClientProvider client={queryClient}>
      <NuqsAdapter>
        <TooltipProvider>
          <React.Suspense>
            <AuthGate>
              <ControlsProvider>
                <SidebarProvider
                  open={navigationOpen}
                  onOpenChange={setNavigationOpen}
                  style={
                    {
                      "--sidebar-width": "calc(var(--spacing) * 60)",
                      "--header-height": "calc(var(--spacing) * 12)",
                    } as React.CSSProperties
                  }
                >
                  <AppSidebar
                    variant="inset"
                    collapsible={isLogs ? "icon" : "offcanvas"}
                  />
                  <SidebarInset
                    className={
                      isLogs
                        ? "h-svh min-w-0 overflow-hidden md:h-[calc(100svh-1rem)]"
                        : "min-w-0"
                    }
                  >
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
