#!/usr/bin/env bun

const proc = Bun.spawn(["bun", "run", "next", "build"], {
    stdout: "inherit",
    stderr: "inherit",
});
await proc.exited;
