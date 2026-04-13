/// Build script — downloads and embeds DuckDB extensions as gzipped bytes.
///
/// Extensions are fetched from extensions.duckdb.org at compile time and embedded
/// in the binary via `include_bytes!`. At runtime they're extracted to disk once,
/// eliminating all network dependencies for extension loading.
///
/// Set SIGIRO_OFFLINE=1 to skip downloads (requires cached extensions in OUT_DIR).
use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::Read;
use std::path::Path;

const DUCKDB_VERSION: &str = "v1.5.0";

struct Extension {
    /// SQL name used in LOAD statements
    name: &'static str,
    /// Actual filename on extensions.duckdb.org (may differ from name)
    filename: &'static str,
    /// true = community-extensions.duckdb.org, false = extensions.duckdb.org
    is_community: bool,
}

const EXTENSIONS: &[Extension] = &[
    // Core extensions (for DuckLake storage)
    Extension {
        name: "ducklake",
        filename: "ducklake",
        is_community: false,
    },
    Extension {
        name: "sqlite",
        filename: "sqlite_scanner",
        is_community: false,
    },
    // Fleet observability extensions
    Extension {
        name: "system_stats",
        filename: "system_stats",
        is_community: true,
    },
    Extension {
        name: "fsquery",
        filename: "fsquery",
        is_community: true,
    },
    Extension {
        name: "hostfs",
        filename: "hostfs",
        is_community: true,
    },
];

fn main() {
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set");
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_else(|_| "unknown".into());
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_else(|_| "unknown".into());

    let platform = match (target_os.as_str(), target_arch.as_str()) {
        ("macos", "aarch64") => "osx_arm64",
        ("macos", "x86_64") => "osx_amd64",
        ("linux", "aarch64") => "linux_arm64",
        ("linux", "x86_64") => "linux_amd64",
        _ => panic!("Unsupported target: {target_os}-{target_arch}"),
    };

    let cache_dir = Path::new(&out_dir).join("ext_cache");
    fs::create_dir_all(&cache_dir).expect("failed to create extension cache dir");

    let offline = env::var("SIGIRO_OFFLINE").is_ok_and(|v| v == "1");

    let mut manifest: HashMap<&str, String> = HashMap::new();

    for ext in EXTENSIONS {
        let gz_filename = format!("{}.duckdb_extension.gz", ext.filename);
        let cache_path = cache_dir.join(&gz_filename);

        if !cache_path.exists() {
            if offline {
                panic!(
                    "SIGIRO_OFFLINE=1 but {} not in cache. Run a normal build first.",
                    gz_filename
                );
            }

            let base = if ext.is_community {
                "https://community-extensions.duckdb.org"
            } else {
                "https://extensions.duckdb.org"
            };
            let url = format!("{base}/{DUCKDB_VERSION}/{platform}/{gz_filename}");

            eprintln!(
                "cargo:warning=Downloading {ext_name}: {url}",
                ext_name = ext.name
            );

            let mut response = ureq::get(&url)
                .call()
                .unwrap_or_else(|e| panic!("Failed to download {url}: {e}"));

            let mut buf = Vec::new();
            response
                .body_mut()
                .as_reader()
                .read_to_end(&mut buf)
                .expect("failed to read extension response");
            fs::write(&cache_path, &buf).expect("failed to cache extension");
        }

        // Store relative path as manifest value
        manifest.insert(ext.name, gz_filename);
    }

    // Write manifest: maps SQL extension name -> gz filename
    let manifest_path = Path::new(&out_dir).join("ext_manifest.rs");
    let mut code = String::from("pub static EXTENSIONS: &[(&str, &[u8])] = &[\n");
    for ext in EXTENSIONS {
        let gz_filename = manifest.get(ext.name).unwrap();
        let cache_path = cache_dir.join(gz_filename);
        code.push_str(&format!(
            "    ({:?}, include_bytes!(concat!(env!(\"OUT_DIR\"), \"/ext_cache/{gz_filename}\"))),\n",
            ext.name,
        ));
        // Re-run if cache file changes
        println!("cargo:rerun-if-changed={}", cache_path.display());
    }
    code.push_str("];\n");
    fs::write(&manifest_path, &code).expect("failed to write ext_manifest.rs");

    println!("cargo:rerun-if-changed=build.rs");
}
