#!/usr/bin/env bun
import { plugin } from "bun";
import { createMdxPlugin } from "fumadocs-mdx/bun";

await plugin(createMdxPlugin());

const proc = Bun.spawn(["bun", "run", "next", "dev"], {
    stdout: "inherit",
    stderr: "inherit",
});
await proc.exited;
