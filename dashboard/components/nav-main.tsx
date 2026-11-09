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
  useSidebar,
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
  const { setOpen, setOpenMobile } = useSidebar()
  // Carry compatible context, without pretending a multi-service explorer
  // selection fits a single-service summary/profile control.
  const params = useSearchParams()
  const keep = new URLSearchParams()
  for (const key of ["range", "live"]) {
    const value = params.get(key)
    if (value) keep.set(key, value)
  }
  const href = (destination: string) => {
    if (destination === pathname) return `${destination}${params.size ? `?${params}` : ""}`
    const next = new URLSearchParams(keep)
    const explorer = (path: string) => ["/logs/", "/traces/"].includes(path)
    const singleService = (path: string) => ["/", "/services/", "/profiles/"].includes(path)
    const compatibleExplorers = explorer(pathname) && explorer(destination)
    if (compatibleExplorers || (singleService(pathname) && singleService(destination))) {
      const service = params.get("service")
      if (service) next.set("service", service)
    }
    if (compatibleExplorers) {
      const timestamp = params.get("timestamp")
      if (timestamp) next.set("timestamp", timestamp)
    }
    return `${destination}${next.size ? `?${next}` : ""}`
  }
  return (
    <SidebarGroup>
      <SidebarGroupContent className="flex flex-col gap-2">
        <SidebarMenu>
          {items.map((item) => (
            <SidebarMenuItem key={item.title}>
              <SidebarMenuButton asChild tooltip={item.title} isActive={pathname === item.url}>
                <Link
                  href={href(item.url)}
                  onClick={() => {
                    setOpenMobile(false)
                    setOpen(false)
                  }}
                  aria-label={item.title}
                  aria-current={pathname === item.url ? "page" : undefined}
                >
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
