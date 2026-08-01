/**
 * The wordmark. There is no logotype, no symbol, no icon — the brackets are the
 * mark. `sigiro` in ink semibold, `[.]` in accent, the TLD in ink-3 medium.
 *
 * The bracketed dot gets positive tracking so the brackets can breathe against
 * the negative tracking around them. This is the one place the accent appears
 * without an interaction behind it, and that is deliberate.
 */
export function Logo({ size = 14 }: { size?: number }) {
  return (
    <span className="inline-block whitespace-nowrap tracking-[-0.02em]" style={{ fontSize: size }}>
      <span className="font-semibold text-ink">sigiro</span>
      <span className="mx-[2.5px] font-semibold tracking-[0.04em] text-accent">[.]</span>
      <span className="font-medium text-ink-3">com</span>
    </span>
  );
}
