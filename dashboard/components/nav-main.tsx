"use client"

import { type Icon } from "@tabler/icons-react"
import Link from "next/link"
import { usePathname, useSearchParams } from "next/navigation"

import {
  SidebarGroup,
  SidebarGroupContent,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
} from "@/components/ui/sidebar"

export function NavMain({
  items,
}: {
  items: {
    title: string
    url: string
    icon?: Icon
  }[]
}) {
  const pathname = usePathname()
  // Carry the header's time range and live switch across pages.
  const params = useSearchParams()
  const keep = new URLSearchParams()
  for (const key of ["range", "live"]) {
    const value = params.get(key)
    if (value) keep.set(key, value)
  }
  const query = keep.size > 0 ? `?${keep}` : ""
  return (
    <SidebarGroup>
      <SidebarGroupContent className="flex flex-col gap-2">
        <SidebarMenu>
          {items.map((item) => (
            <SidebarMenuItem key={item.title}>
              <SidebarMenuButton
                asChild
                tooltip={item.title}
                isActive={pathname === item.url}
              >
                <Link href={`${item.url}${query}`}>
                  {item.icon && <item.icon />}
                  <span>{item.title}</span>
                </Link>
              </SidebarMenuButton>
            </SidebarMenuItem>
          ))}
        </SidebarMenu>
      </SidebarGroupContent>
    </SidebarGroup>
  )
}
