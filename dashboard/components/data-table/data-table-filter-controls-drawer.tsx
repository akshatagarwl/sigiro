import { Button } from "@/components/ui/button";
import {
  Drawer,
  DrawerClose,
  DrawerContent,
  DrawerDescription,
  DrawerFooter,
  DrawerHeader,
  DrawerTitle,
  DrawerTrigger,
} from "@/components/ui/drawer";
import { Kbd } from "@/components/ui/kbd";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useHotKey } from "@/hooks/use-hot-key";
import { useMediaQuery } from "@/hooks/use-media-query";
import { VisuallyHidden } from "@radix-ui/react-visually-hidden";
import { FilterIcon } from "lucide-react";
import React from "react";
import { DataTableFilterControls } from "./data-table-filter-controls";
import { TOOLTIP_DELAY } from "./ui-compat";

export function DataTableFilterControlsDrawer({ showLabel = false, shortcut = true }: { showLabel?: boolean; shortcut?: boolean } = {}) {
  const triggerButtonRef = React.useRef<HTMLButtonElement>(null);
  const isMobile = useMediaQuery("(max-width: 640px)");

  useHotKey(() => {
    if (shortcut) triggerButtonRef.current?.click();
  }, "b");

  return (
    <Drawer>
      <TooltipProvider {...TOOLTIP_DELAY}>
        <Tooltip>
          <TooltipTrigger asChild>
            <DrawerTrigger asChild>
              <Button
                ref={isMobile ? triggerButtonRef : null}
                variant="ghost"
                size={showLabel ? "default" : "icon"}
                aria-label="Filters"
                className={showLabel ? "h-8 gap-2 px-3" : "h-9 w-9"}
              >
                <FilterIcon className="h-4 w-4" aria-hidden="true" />
                {showLabel && "Filters"}
              </Button>
            </DrawerTrigger>
          </TooltipTrigger>
          <TooltipContent side="right">
            <p className="text-nowrap">
              {shortcut ? "Toggle controls with " : "Open filters"}{" "}
              {shortcut && <Kbd className="text-muted-foreground group-hover:text-accent-foreground ml-1">
                <span className="mr-1">⌘</span>
                <span>B</span>
              </Kbd>}
            </p>
          </TooltipContent>
        </Tooltip>
      </TooltipProvider>
      <DrawerContent className="max-h-[calc(100dvh-4rem)]">
        <VisuallyHidden>
          <DrawerHeader>
            <DrawerTitle>Filters</DrawerTitle>
            <DrawerDescription>Adjust your table filters</DrawerDescription>
          </DrawerHeader>
        </VisuallyHidden>
        <div className="flex-1 overflow-y-auto px-4">
          <DataTableFilterControls />
        </div>
        <DrawerFooter>
          <DrawerClose asChild>
            <Button variant="outline" className="w-full">
              Close
            </Button>
          </DrawerClose>
        </DrawerFooter>
      </DrawerContent>
    </Drawer>
  );
}
