const std = @import("std");

pub fn build(b: *std.Build) void {
    // ── options ────────────────────────────────────────────────
    const target = b.option([]const u8, "target", "Cross-compilation target (e.g. x86_64-linux, aarch64-linux)");

    // ── sigiro CLI ────────────────────────────────────────────
    const sigiro = addGoBuild(b, "sigiro", "./cmd/sigiro/", target);
    b.default_step.dependOn(&sigiro.step);

    // ── agents.txt ────────────────────────────────────────────
    const agents_txt = b.addSystemCommand(&.{ "dist/sigiro", "schema" });
    agents_txt.step.dependOn(&sigiro.step);
    const agents_step = b.step("agents.txt", "Generate agents.txt from schema registry");
    const output = agents_txt.captureStdOut();
    const install_agents = b.addInstallFile(output, "../agents.txt");
    agents_step.dependOn(&install_agents.step);

    // ── collector ─────────────────────────────────────────────
    const collector_step = b.step("collector", "Build OTel collector");
    const ocb_install = b.addSystemCommand(&.{
        "go", "install", "go.opentelemetry.io/collector/cmd/builder@v0.147.0",
    });
    const ocb_build = b.addSystemCommand(&.{ "builder", "--config=otelcol-builder.yaml" });
    ocb_build.step.dependOn(&ocb_install.step);
    collector_step.dependOn(&ocb_build.step);

    // ── test ──────────────────────────────────────────────────
    const test_step = b.step("test", "Run Go tests");
    const go_test = b.addSystemCommand(&.{ "go", "test", "./..." });
    test_step.dependOn(&go_test.step);

    // ── e2e ───────────────────────────────────────────────────
    const e2e_step = b.step("e2e", "Run e2e tests");
    const e2e_cmd = b.addSystemCommand(&.{ "bash", "test/e2e/run.sh" });
    e2e_cmd.setEnvironmentVariable("DOCKER_BUILDKIT", "1");
    e2e_step.dependOn(&e2e_cmd.step);

    const e2e_real_step = b.step("e2e-real", "Run real e2e tests");
    const e2e_real_cmd = b.addSystemCommand(&.{ "bash", "test/e2e/run-real.sh" });
    e2e_real_cmd.setEnvironmentVariable("DOCKER_BUILDKIT", "1");
    e2e_real_step.dependOn(&e2e_real_cmd.step);

    const sdk_step = b.step("sdk-validate", "Run SDK validation tests");
    const sdk_cmd = b.addSystemCommand(&.{ "bash", "test/sdk-validation/run.sh" });
    sdk_cmd.setEnvironmentVariable("DOCKER_BUILDKIT", "1");
    sdk_step.dependOn(&sdk_cmd.step);

    // ── all platforms ─────────────────────────────────────────
    const all_step = b.step("all", "Build for all platforms (darwin-arm64, linux-amd64, linux-arm64)");
    const native = addGoBuild(b, "sigiro", "./cmd/sigiro/", null);
    const linux_amd64 = addGoBuild(b, "sigiro-linux-amd64", "./cmd/sigiro/", "x86_64-linux");
    const linux_arm64 = addGoBuild(b, "sigiro-linux-arm64", "./cmd/sigiro/", "aarch64-linux");
    all_step.dependOn(&native.step);
    all_step.dependOn(&linux_amd64.step);
    all_step.dependOn(&linux_arm64.step);

    // ── clean ─────────────────────────────────────────────────
    const clean_step = b.step("clean", "Remove build artifacts");
    const clean_cmd = b.addSystemCommand(&.{ "rm", "-rf", "dist/", "zig-out/" });
    clean_step.dependOn(&clean_cmd.step);
}

// addGoBuild creates a `go build` step with the right CC/CXX for the target.
//
// Cross-compilation strategy:
//   - native (null target):     zig cc / zig c++
//   - x86_64-linux:             x86_64-linux-musl-gcc / g++ (brew install FiloSottile/musl-cross/musl-cross)
//   - aarch64-linux:            aarch64-linux-musl-gcc / g++
//
// Why musl-cross for Linux instead of zig cc?
// DuckDB's pre-compiled .a files are built with GCC (libstdc++ ABI).
// Zig ships libc++ (LLVM ABI). These ABIs are incompatible — linking
// a GCC-compiled .a with zig's libc++ produces undefined symbol errors.
// musl-cross provides GCC + libstdc++ for the target, matching DuckDB's ABI.
//
// DuckDB's .a was compiled against glibc but we link with musl. Three glibc-only
// functions (backtrace_symbols, malloc_trim, __res_init) are stubbed in
// cmd/sigiro/musl_stubs.c, compiled only when CGO_CFLAGS includes -DMUSL_STUBS.
fn addGoBuild(
    b: *std.Build,
    name: []const u8,
    pkg: []const u8,
    target: ?[]const u8,
) *std.Build.Step.Run {
    const output_path = b.fmt("dist/{s}", .{name});

    const cmd = b.addSystemCommand(&.{
        "go", "build", "-trimpath", "-o", output_path, pkg,
    });

    cmd.setEnvironmentVariable("CGO_ENABLED", "1");

    if (target) |t| {
        if (std.mem.eql(u8, t, "x86_64-linux")) {
            cmd.setEnvironmentVariable("GOOS", "linux");
            cmd.setEnvironmentVariable("GOARCH", "amd64");
            cmd.setEnvironmentVariable("CC", "x86_64-linux-musl-gcc");
            cmd.setEnvironmentVariable("CXX", "x86_64-linux-musl-g++");
            cmd.setEnvironmentVariable("CGO_CFLAGS", "-DMUSL_STUBS");
            cmd.setEnvironmentVariable("CGO_LDFLAGS", "-static");
        } else if (std.mem.eql(u8, t, "aarch64-linux")) {
            cmd.setEnvironmentVariable("GOOS", "linux");
            cmd.setEnvironmentVariable("GOARCH", "arm64");
            cmd.setEnvironmentVariable("CC", "aarch64-linux-musl-gcc");
            cmd.setEnvironmentVariable("CXX", "aarch64-linux-musl-g++");
            cmd.setEnvironmentVariable("CGO_CFLAGS", "-DMUSL_STUBS");
            cmd.setEnvironmentVariable("CGO_LDFLAGS", "-static");
        } else {
            @panic("unsupported target — use x86_64-linux or aarch64-linux");
        }
    } else {
        // Native build: use zig cc (drops libc++ dependency)
        cmd.setEnvironmentVariable("CC", "zig cc");
        cmd.setEnvironmentVariable("CXX", "zig c++");
    }

    return cmd;
}
