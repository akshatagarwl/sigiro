import { createHighlighterCore } from 'shiki/core';
import { createJavaScriptRegexEngine } from 'shiki/engine/javascript';
import bash from 'shiki/langs/bash.mjs';
import ts from 'shiki/langs/typescript.mjs';
import yaml from 'shiki/langs/yaml.mjs';
import json from 'shiki/langs/json.mjs';
import sql from 'shiki/langs/sql.mjs';
import githubLight from 'shiki/themes/github-light-default.mjs';

export type Lang = 'json' | 'sql' | 'bash' | 'typescript' | 'yaml';

/**
 * Shiki, built for the Workers runtime.
 *
 * The convenient `codeToHtml` from `shiki` pulls the oniguruma WASM engine, which
 * throws in Cloudflare Workers — the page 500s there while rendering fine under
 * Node. `createJavaScriptRegexEngine` uses the platform's own RegExp instead, and
 * the fine-grained imports keep only the three grammars and one theme we use
 * rather than every language Shiki ships.
 *
 * Note this runs per request, not at build: vinext reports these routes as
 * static but does not emit HTML files, so the Worker renders them. Hence the
 * module-level singleton — the highlighter is built once per isolate, not once
 * per request.
 */
const highlighter = createHighlighterCore({
  themes: [githubLight],
  langs: [json, sql, bash, ts, yaml],
  engine: createJavaScriptRegexEngine(),
});

const LANGS = new Set<string>(['json', 'sql', 'bash', 'typescript', 'yaml']);

/** The aliases the docs actually use in fenced blocks. */
const ALIAS: Record<string, Lang> = {
  sh: 'bash',
  shell: 'bash',
  console: 'bash',
  js: 'typescript',
  ts: 'typescript',
  tsx: 'typescript',
  yml: 'yaml',
};

/** Null for a fence we have no grammar for — render it plain rather than throw. */
export function resolveLang(lang: string | undefined): Lang | null {
  if (!lang) return null;
  const l = lang.toLowerCase();
  if (LANGS.has(l)) return l as Lang;
  return ALIAS[l] ?? null;
}

/**
 * Two of the theme's token colours land just under AA at our code size — the
 * comment grey at 4.0:1 and the keyword red at 4.49:1 against 4.5 required.
 * `colorReplacements` is Shiki's own hook for this, so the theme is adjusted
 * rather than the output patched afterwards.
 */
const COLOR_REPLACEMENTS = { '#6e7781': '#5a6169', '#cf222e': '#b3121f' };

export async function highlight(code: string, lang: Lang): Promise<string> {
  const shiki = await highlighter;
  return shiki.codeToHtml(code, {
    lang,
    theme: 'github-light-default',
    colorReplacements: COLOR_REPLACEMENTS,
  });
}
