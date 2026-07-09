#!/usr/bin/env bun

const proc = Bun.spawn(["bun", "run", "next", "dev"], {
    stdout: "inherit",
    stderr: "inherit",
});
await proc.exited;
