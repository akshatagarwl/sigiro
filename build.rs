use std::env;

// `CARGO_CFG_TARGET_OS` is Cargo's documented interface to a build script, not
// application configuration, so the typed-env indirection `clippy.toml` requires
// elsewhere does not apply: this variable is set by Cargo for this process and
// is unreadable from the built binary.
#[allow(clippy::disallowed_methods)]
fn main() {
    // `sigiro --version`: the release driver sets SIGIRO_BUILD_COMMIT to the
    // short source SHA so a downloaded binary names the commit it came from.
    // Every release is published as the mutable 0.1.0, so the package version
    // alone cannot tell two downloads apart.
    println!("cargo:rerun-if-env-changed=SIGIRO_BUILD_COMMIT");
    let version = env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let long = match env::var("SIGIRO_BUILD_COMMIT") {
        Ok(commit) if !commit.trim().is_empty() => format!("{version} ({})", commit.trim()),
        _ => version,
    };
    println!("cargo:rustc-env=SIGIRO_VERSION={long}");

    dashboard();

    match env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("linux") => {
            println!("cargo:rustc-link-lib=dylib=dl");
            println!("cargo:rustc-link-lib=dylib=pthread");
            println!("cargo:rustc-link-lib=dylib=stdc++");
        }
        Ok("macos") => println!("cargo:rustc-link-arg=-lc++"),
        _ => {}
    }
}

/// `src/dashboard.rs` embeds `dashboard/dist/dashboard`, the dashboard's static
/// export. Release builds set `SIGIRO_REQUIRE_DASHBOARD=1` so a binary cannot
/// ship without its UI; other builds embed whatever is there, or nothing.
#[allow(clippy::disallowed_methods)]
fn dashboard() {
    let export = std::path::Path::new("dashboard/dist/dashboard");
    println!("cargo:rerun-if-env-changed=SIGIRO_REQUIRE_DASHBOARD");
    // Rebuild when the export changes. `dashboard-build` deletes and recreates
    // `dashboard/dist`, so watching it catches a first build too. A watched
    // path that does not exist would make every build dirty, so make sure the
    // (gitignored) directory is there; a read-only tree just goes unwatched.
    if std::fs::create_dir_all("dashboard/dist").is_ok() {
        println!("cargo:rerun-if-changed=dashboard/dist");
    }
    let required = env::var("SIGIRO_REQUIRE_DASHBOARD").is_ok_and(|v| v == "1");
    if required && !export.join("index.html").is_file() {
        panic!(
            "SIGIRO_REQUIRE_DASHBOARD=1 but dashboard/dist/dashboard/index.html is missing. \
             Build the dashboard first: `mise run dashboard-install dashboard-build` \
             (or use `mise run build`, which does both)."
        );
    }
}
