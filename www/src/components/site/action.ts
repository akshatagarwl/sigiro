/**
 * The two actions this site has, as class strings rather than a component.
 *
 * A `<Button render={<Link/>}>` wrapper existed to paper over the fact that one
 * of these is an anchor and the other is a button — but every action on this site
 * is a link, so the wrapper was one indirection and a `cva` dependency for two
 * strings. The design language spends its buttons the same way: the classes go on
 * the element.
 *
 * Radius 9 is the button radius. `press` is the 0.12s translateY(1px).
 *
 * PRIMARY is an ink fill — an inverted surface on a light page — so its focus
 * mark is `#93B0FF`, the dark-mode accent, in light mode. That is not a bug: the
 * accent is chosen against the surface it is drawn on, not against the page.
 */
const BASE =
  "press inline-flex shrink-0 items-center justify-center gap-2 rounded-[9px] text-[13px] font-medium whitespace-nowrap outline-none";

export const PRIMARY = `${BASE} h-9 bg-ink px-3.5 text-panel focus-visible:outline-none focus-visible:shadow-[0_0_0_1.5px_#93B0FF] dark:focus-visible:shadow-[0_0_0_1.5px_#4568FF]`;

export const SECONDARY = `${BASE} mat-cap h-9 px-3.5 text-ink-2 hover:text-ink focus-visible:outline-none focus-visible:shadow-[0_1.5px_0_rgba(28,25,23,0.08),inset_0_0_0_1px_var(--accent),0_10px_20px_-14px_rgba(69,104,255,0.6)]`;

/** The same cap at the header's height, for a nav-bar action. */
export const COMPACT = `${BASE.replace("text-[13px]", "text-[12.5px]")} mat-cap h-7 px-2.5 text-ink-2 hover:text-ink focus-visible:outline-none focus-visible:shadow-[0_1.5px_0_rgba(28,25,23,0.08),inset_0_0_0_1px_var(--accent),0_10px_20px_-14px_rgba(69,104,255,0.6)]`;
