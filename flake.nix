{
  description = "Sigiro — observability pipeline with DuckDB/DuckLake storage";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane = {
      url = "github:ipetkov/crane";
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay, crane }:
    flake-utils.lib.eachSystem [
      "aarch64-darwin"  # macOS ARM64 (build machine)
      "x86_64-linux"    # Linux x86_64 (CI)
    ] (buildSystem:
      let
        targetTriple = "aarch64-unknown-linux-gnu";

        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          system = buildSystem;
          inherit overlays;
        };

        lib = pkgs.lib;

        # Cross-compilation sysroot — GCC toolchain + glibc for aarch64-linux.
        # glibc ~2.39 from nixpkgs-unstable, forward-compatible with all newer systems.
        crossPkgs = pkgs.pkgsCross.aarch64-multiplatform;

        # Rust toolchain — latest stable with aarch64-linux cross-compilation target
        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          targets = [ targetTriple ];
        };

        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

        crossPrefix = crossPkgs.stdenv.cc.targetPrefix;

        src = craneLib.cleanCargoSource ./.;

        # Common env vars for cross-compilation.
        # Uses GCC (not clang) so bundled DuckDB C++ is ABI-compatible
        # with community DuckDB extensions (both use libstdc++).
        crossEnv = {
          CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER = "${crossPrefix}gcc";
          CC_aarch64_unknown_linux_gnu  = "${crossPrefix}gcc";
          CXX_aarch64_unknown_linux_gnu = "${crossPrefix}g++";
          AR_aarch64_unknown_linux_gnu  = "${crossPrefix}ar";
          CXXFLAGS_aarch64_unknown_linux_gnu = "-std=c++17";
          PKG_CONFIG_ALLOW_CROSS = "1";
        };

        # Build tools that run on the HOST machine
        nativeBuildInputs = with pkgs; [
          cmake
          pkg-config
          python3
          crossPkgs.stdenv.cc
          crossPkgs.stdenv.cc.bintools
        ];

        # ---- Dependency derivation (cached by Nix, rebuilds only on Cargo.lock change) ----
        # This is the key optimization: DuckDB C++ compilation happens HERE and is cached.
        # Source changes only rebuild the second derivation below.

        nativeDeps = craneLib.buildDepsOnly {
          inherit src;
          strictDeps = true;
          nativeBuildInputs = with pkgs; [ cmake pkg-config python3 ];

          preBuild = ''
            ulimit -n 65536 2>/dev/null || true
          '';
        };

        crossDeps = craneLib.buildDepsOnly {
          inherit src;
          strictDeps = true;

          cargoExtraArgs = "--target ${targetTriple}";
          inherit nativeBuildInputs;

          env = crossEnv;

          preBuild = ''
            ulimit -n 65536 2>/dev/null || true
          '';
        };

        # ---- Native build (for the build machine's own architecture) ----
        nativeBuild = craneLib.buildPackage {
          inherit src;
          strictDeps = true;
          nativeBuildInputs = with pkgs; [ cmake pkg-config python3 ];

          cargoArtifacts = nativeDeps;

          preBuild = ''
            ulimit -n 65536 2>/dev/null || true
          '';

          preCheck = ''
            export HOME=$(mktemp -d)
          '';
        };

        # ---- Cross-compiled build (aarch64-linux-gnu) ----
        crossBuild = craneLib.buildPackage {
          inherit src;
          strictDeps = true;

          cargoArtifacts = crossDeps;
          cargoExtraArgs = "--target ${targetTriple}";
          inherit nativeBuildInputs;

          env = crossEnv;

          preBuild = ''
            ulimit -n 65536 2>/dev/null || true
          '';

          doCheck = false;

          installPhase = ''
            mkdir -p $out/bin
            cp target/${targetTriple}/release/sigiro $out/bin/sigiro
          '';

          postFixup = ''
            echo "--- Cross-compiled binary info ---"
            ${pkgs.buildPackages.file}/bin/file $out/bin/sigiro
            ${pkgs.buildPackages.binutils}/bin/${crossPrefix}readelf -d $out/bin/sigiro 2>/dev/null || true
          '';
        };

      in {
        packages = {
          default = nativeBuild;
          sigiro = nativeBuild;
          sigiro-aarch64-linux = crossBuild;
        };

        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            # Rust toolchain (with aarch64-linux target)
            rustToolchain

            # Build dependencies
            cmake
            pkg-config
            python3

            # Cross-compilation toolchain (GCC for aarch64-linux)
            # Ensures DuckDB C++ is compiled with GCC/libstdc++ for ABI compat
            crossPkgs.stdenv.cc
            crossPkgs.stdenv.cc.bintools

            # Dev tools
            cargo-watch
            cargo-outdated
            rust-analyzer
            gdb
          ];

          env = crossEnv;

          shellHook = ''
            echo "sigiro dev shell"
            echo "  rustc: $(rustc --version)"
            echo "  cross: ${crossPrefix}gcc $(${crossPrefix}gcc -dumpversion 2>/dev/null)"
            echo "  target: ${targetTriple}"
            echo ""
            echo "  nix build .#sigiro-aarch64-linux   — cross-compile for aarch64 Linux"
            echo "  nix build                           — native build"
          '';
        };
      }
    );
}
