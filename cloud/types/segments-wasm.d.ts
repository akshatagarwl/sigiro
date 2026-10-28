declare module "../vendor/segments-wasm/sigiro_segments_wasm.js" {
  export default function init(input: WebAssembly.Module): Promise<unknown>;
  export function decode_otlp(
    signal: string,
    format: string,
    body: Uint8Array
  ): Array<{ metadata: string; parquet: Uint8Array }>;
  export function manifest_empty(): string;
  export function manifest_register(
    manifest: string,
    key: string,
    table: string
  ): string;
  export function manifest_commit(manifest: string, segment: string): string;
  export function manifest_live(
    manifest: string,
    table: string,
    fromUs: bigint | number,
    toUs: bigint | number
  ): string;
  export function manifest_orphans(
    manifest: string,
    stored: string[]
  ): string[];
  export function empty_segment(table: string): Uint8Array;
  export function validate_read_only(sql: string): void;
}

declare module "*.wasm" {
  const module: WebAssembly.Module;
  export default module;
}
