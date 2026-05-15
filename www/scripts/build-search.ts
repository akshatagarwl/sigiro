#!/usr/bin/env bun
import { source } from "./src/app/source";
import { Document } from "flexsearch";
import { join } from "node:path";
import { homedir } from "node:os";

// Create document with SAME settings as fumadocs
const document = new Document({
  tokenize: "full",
  document: {
    id: "id",
    index: ["content"],
    tag: ["tags"],
    store: true
  }
});

const pages = source.getPages();
console.log(`Indexing ${pages.length} pages...`);

for (const page of pages) {
  const structuredData = page.data.structuredData;
  
  if (structuredData) {
    let id = 0;
    const nextId = () => `${page.url}-${id++}`;
    
    // Index the page title
    document.add({
      id: page.url,
      page_id: page.url,
      type: "page",
      content: page.data.title || "",
      breadcrumbs: [],
      tags: [],
      url: page.url
    });
    
    // Index each heading
    for (const heading of structuredData.headings || []) {
      document.add({
        id: nextId(),
        page_id: page.url,
        type: "heading",
        content: heading.content,
        breadcrumbs: [],
        tags: [],
        url: `${page.url}#${heading.id}`
      });
    }
    
    // Index first few content items (not all to save space)
    for (const content of (structuredData.contents || []).slice(0, 3)) {
      document.add({
        id: nextId(),
        page_id: page.url,
        type: "text",
        content: content.content,
        breadcrumbs: [],
        tags: [],
        url: `${page.url}#${content.heading}`
      });
    }
  }
}

// Export the index
const raw: Record<string, any> = {};
await document.export((key: string, data: any) => {
  raw[key] = data;
});

const exported = {
  type: "default",
  raw
};

const cacheDir = join(homedir(), ".cache", "sigiro");
await Bun.write(join(cacheDir, "search-index.json"), JSON.stringify(exported));

console.log("Search index built!");
console.log("Pages indexed:", pages.length);
console.log("File size:", JSON.stringify(exported).length, "bytes");
