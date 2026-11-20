#[cfg(feature = "package-env")]
pub const PACKAGE_NAME: &str = env!("CARGO_PKG_NAME");

#[cfg(all(feature = "rustflags", not(required_by_repro)))]
compile_error!("RUSTFLAGS --cfg required_by_repro was not forwarded");

pub fn answer() -> u32 {
    42
}
