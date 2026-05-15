import { plugin } from "bun";
import { createMdxPlugin } from "fumadocs-mdx/bun";

await plugin(createMdxPlugin());
