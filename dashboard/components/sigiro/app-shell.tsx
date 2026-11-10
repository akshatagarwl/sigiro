"use client";

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { usePathname } from "next/navigation";
import { NuqsAdapter } from "nuqs/adapters/next/app";
import * as React from "react";

import { AppSidebar } from "@/components/app-sidebar";
import { AlertDeliveryNotice } from "@/components/sigiro/alert-delivery-notice";
import { AuthGate } from "@/components/sigiro/auth-gate";
import { SiteHeader } from "@/components/site-header";
import { SidebarInset, SidebarProvider } from "@/components/ui/sidebar";
import { Toaster } from "@/components/ui/sonner";
import { TooltipProvider } from "@/components/ui/tooltip";
import { ControlsProvider } from "@/lib/sigiro/controls";

/** dashboard-01's sidebar + inset layout, behind sign-in, with the providers the blocks need. */
export function AppShell({ children }: { children: React.ReactNode }) {
  const [queryClient] = React.useState(() => new QueryClient());
  const pathname = usePathname();
  const [navigationOpen, setNavigationOpen] = React.useState(false);
  React.useEffect(() => setNavigationOpen(false), [pathname]);
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
                  <AppSidebar variant="inset" collapsible="icon" />
                  <SidebarInset className="h-svh min-w-0 overflow-hidden md:h-[calc(100svh-1rem)]">
                    <SiteHeader />
                    <AlertDeliveryNotice />
                    <div className="@container/main flex min-h-0 flex-1 flex-col overflow-y-auto">
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
