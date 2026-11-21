fn main() {
    #[cfg(feature = "build-env")]
    for name in ["DEBUG", "PROFILE", "NUM_JOBS"] {
        let value = std::env::var(name)
            .unwrap_or_else(|error| panic!("Cargo build-script variable {name} missing: {error}"));
        println!("cargo:warning={name}={value}");
    }
}
