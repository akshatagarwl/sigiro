/**
 * The wordmark. There is no logotype and no symbol — the name, set once,
 * lowercase, is the mark.
 *
 * It used to be `sigiro[.]com`, with the bracketed dot in accent. That was
 * interior.dev's mark with our name in it, down to the bracket pair and the
 * greyed TLD, so it identified the design language rather than us. It also
 * spent the accent on decoration, which is the one thing the accent is not for.
 */
export function Logo({ size = 14 }: { size?: number }) {
  return (
    <span
      className="text-ink inline-block font-semibold tracking-[-0.02em] whitespace-nowrap"
      style={{ fontSize: size }}
    >
      sigiro
    </span>
  );
}
