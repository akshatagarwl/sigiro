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
