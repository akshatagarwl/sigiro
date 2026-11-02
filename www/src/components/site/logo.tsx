/**
 * The wordmark. There is no logotype and no symbol — the name, set once,
 * lowercase, is the mark.
 */
export function Logo({ size = 14 }: { size?: number }) {
  return (
    <span
      className="text-foreground inline-block font-semibold tracking-[-0.02em] whitespace-nowrap"
      style={{ fontSize: size }}
    >
      sigiro
    </span>
  );
}
