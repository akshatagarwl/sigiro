use std::{env, fs, path::PathBuf};
// Cargo's build-time OUT_DIR is not Sigiro runtime environment configuration.
#[allow(clippy::disallowed_methods)]
fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::write(out.join("memory.x"), "MEMORY { FLASH : ORIGIN = 0x00000000, LENGTH = 256K\nRAM : ORIGIN = 0x20000000, LENGTH = 64K }\n").unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=build.rs");
}
