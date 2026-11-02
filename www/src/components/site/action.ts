import { buttonVariants } from "@/components/ui/button";

/**
 * Link-shaped actions. Every action on the landing page is an anchor, so these
 * are shadcn's button classes rather than a `<Button asChild>` wrapper.
 */
export const PRIMARY = buttonVariants();

export const SECONDARY = buttonVariants({ variant: "outline" });

/** The header's smaller action. */
export const COMPACT = buttonVariants({ variant: "outline", size: "sm" });
