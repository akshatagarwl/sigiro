import { NextRequest } from "next/server";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { homedir } from "node:os";

// Dynamic import of flexsearch
async function getDocument() {
  const { Document } = await import("flexsearch");
  
  const filePath = join(homedir(), ".cache", "sigiro", "search-index.json");
  const data = JSON.parse(await readFile(filePath, "utf-8"));
  
  const document = new Document({
    tokenize: "full",
    document: {
      id: "id",
      index: ["content"],
      tag: ["tags"],
      store: true
    }
  });
  
  if (data.raw) {
    for (const [key, value] of Object.entries(data.raw)) {
      await document.import(key, value as any);
    }
  }
  
  return document;
}

let documentPromise: ReturnType<typeof getDocument> | null = null;

async function getCachedDocument() {
  if (!documentPromise) {
    documentPromise = getDocument();
  }
  return documentPromise;
}

export async function GET(request: NextRequest) {
  const query = request.nextUrl.searchParams.get("query");
  
  if (!query) {
    return Response.json([]);
  }
  
  try {
    const document = await getCachedDocument();
    const results = await document.searchAsync(query, { limit: 10 });
    
    // Flatten and format results
    const seen = new Set();
    const flatResults = results
      .flatMap((result: any) => result.result)
      .filter((id: string) => {
        if (seen.has(id)) return false;
        seen.add(id);
        return true;
      })
      .slice(0, 10);
    
    // Fetch document details for each result
    const docs = await Promise.all(
      flatResults.map(async (id: string) => {
        const doc = await document.get(id);
        return doc || { id, url: id, content: "", type: "page" };
      })
    );
    
    return Response.json(docs);
  } catch (error) {
    console.error("Search error:", error);
    return Response.json([]);
  }
}
