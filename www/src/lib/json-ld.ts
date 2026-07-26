/**
 * JSON-LD embedded in a <script> must not be able to close it. `</script>` inside
 * any string — a doc title, a description — ends the element and everything after
 * becomes markup. Escaping `<` covers that and the `<!--` case, and is still valid
 * JSON, so parsers read it unchanged.
 */
export function jsonLd(data: unknown): string {
  return JSON.stringify(data).replace(/</g, '\\u003c');
}
