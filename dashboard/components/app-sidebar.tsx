"use client"

import * as React from "react"
import {
  IconBook,
  IconDashboard,
  IconDatabase,
  IconFlame,
  IconHierarchy2,
  IconListDetails,
  IconServer,
} from "@tabler/icons-react"

import { NavMain } from "@/components/nav-main"
import { NavSecondary } from "@/components/nav-secondary"
import { NavUser } from "@/components/nav-user"
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
} from "@/components/ui/sidebar"

const data = {
  navMain: [
    { title: "Overview", url: "/", icon: IconDashboard },
    { title: "Services", url: "/services/", icon: IconServer },
    { title: "Traces", url: "/traces/", icon: IconHierarchy2 },
    { title: "Logs", url: "/logs/", icon: IconListDetails },
    { title: "Profiles", url: "/profiles/", icon: IconFlame },
    { title: "SQL", url: "/sql/", icon: IconDatabase },
  ],
  navSecondary: [{ title: "Docs", url: "/docs", icon: IconBook }],
}

export function AppSidebar({ ...props }: React.ComponentProps<typeof Sidebar>) {
  return (
    <Sidebar collapsible="offcanvas" {...props}>
      <SidebarHeader>
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton
              asChild
              className="data-[slot=sidebar-menu-button]:p-1.5!"
            >
              <a href="/">
                <span className="text-base font-semibold">Sigiro</span>
              </a>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarHeader>
      <SidebarContent>
        <NavMain items={data.navMain} />
        <NavSecondary items={data.navSecondary} className="mt-auto" />
      </SidebarContent>
      <SidebarFooter>
        <NavUser />
      </SidebarFooter>
    </Sidebar>
  )
}
