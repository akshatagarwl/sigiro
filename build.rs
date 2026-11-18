use std::env;

// `CARGO_CFG_TARGET_OS` is Cargo's documented interface to a build script, not
// application configuration, so the typed-env indirection `clippy.toml` requires
// elsewhere does not apply: this variable is set by Cargo for this process and
// is unreadable from the built binary.
#[allow(clippy::disallowed_methods)]
fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }

    // libduckdb-sys emits the static DuckDB archive itself. A C++ static
    // library also needs its platform runtime dependencies at the final link.
    println!("cargo:rustc-link-lib=dylib=dl");
    println!("cargo:rustc-link-lib=dylib=pthread");
    println!("cargo:rustc-link-lib=dylib=stdc++");
}
