{
  description = "sigiro: a single-binary OpenTelemetry backend";

  # Sigiro outputs and prebuilt DuckDB come from signed binary caches. Nix
  # honours this for trusted users; otherwise add the settings to nix.conf
  # (CONTRIBUTING.md).
  nixConfig = {
    extra-substituters = [
      "https://akshat-sigiro-builds.cachix.org"
      "https://github.com/akshatagarwl/duckdb-static/releases/download/nix-cache"
    ];
    extra-trusted-public-keys = [
      "akshat-sigiro-builds.cachix.org-1:YjaKP3P+EoMjSjDAN1yI5U5dbZr5l9yNI5qeFphJvA8="
      "duckdb-static.github.akshatagarwl-1:1yQ0flJ3vpCoIicGqSTO8gHezxiCOj2M0VLyJXXqQVQ="
    ];
  };

  inputs = {
    # DuckDB with every extension sigiro uses compiled in, as one static
    # archive. sigiro follows its nixpkgs: the archive is then a cache hit, and
    # sigiro links it with the same static toolchain and C++ runtime.
    duckdb-static.url = "github:akshatagarwl/duckdb-static";
    nixpkgs.follows = "duckdb-static/nixpkgs";
    crane.url = "github:ipetkov/crane";
    cargo-dyndrv = {
      url = "github:obsidiansystems/cargo-dyndrv/d5245b85f82627fcaf041e2c57c8aeb7d57d20fe";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      duckdb-static,
      crane,
      cargo-dyndrv,
      rust-overlay,
    }:
    let
      inherit (nixpkgs) lib;

      # The release hosts (release.yml) that nixpkgs still supports: it dropped
      # x86_64-darwin in 26.11, so that target stays a mise/CI build.
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];

      cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
      version = cargoToml.package.version;
      toolchainFile = (builtins.fromTOML (builtins.readFile ./rust-toolchain.toml)).toolchain;

      perSystem =
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [
              rust-overlay.overlays.default
              cargo-dyndrv.overlays.default
            ];
            # FSL-1.1-ALv2 is source-available, not free, and is marked so.
            config.allowUnfreePredicate = pkg: lib.getName pkg == "sigiro";
          };

          # The binary is built for nixpkgs' static platform: musl on Linux
          # (fully static), static libraries on macOS (see nix/rust.nix).
          staticTarget = pkgs.pkgsStatic.stdenv.hostPlatform.rust.rustcTarget;

          # The pinned toolchain (rust-toolchain.toml) with the static target,
          # and wasm32 for crates/segments-wasm.
          buildToolchain = pkgs.rust-bin.stable.${toolchainFile.channel}.minimal.override {
            extensions = [
              "clippy"
              "rustfmt"
            ];
            targets = [
              staticTarget
              "wasm32-unknown-unknown"
            ];
          };
          # For the dev shell: everything rust-toolchain.toml lists, plus
          # editor support.
          devToolchain = (pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml).override {
            extensions = [
              "clippy"
              "rustfmt"
              "rust-src"
              "rust-analyzer"
            ];
          };
          craneLib = (crane.mkLib pkgs).overrideToolchain (_: buildToolchain);

          duckdb = duckdb-static.packages.${system}.default;
          aube = pkgs.callPackage ./nix/aube.nix { };

          rust = pkgs.callPackage ./nix/rust.nix { inherit craneLib duckdb version; };
          benchmark = pkgs.callPackage ./nix/dyndrv-benchmark.nix {
            inherit
              rust
              craneLib
              buildToolchain
              version
              ;
          };
          repros = pkgs.callPackage ./nix/dyndrv-repros.nix { inherit craneLib; };
          dashboard = pkgs.callPackage ./nix/dashboard.nix { inherit version; };
          docs = pkgs.callPackage ./nix/docs.nix {
            inherit version;
            inherit (rust) openapi;
          };
          devPackages = [
            devToolchain
            aube
            pkgs.nodejs_24
            pkgs.cargo-nextest
          ];
          devEnv = rust.targetEnv // {
            CARGO_PROFILE_DEV_INCREMENTAL = "true";
            CARGO_PROFILE_TEST_INCREMENTAL = "true";
          };
          linkerShell =
            linker:
            let
              cc = pkgs.pkgsStatic.stdenv.cc;
              # Give GCC a supported linker name and the static-target prefix;
              # the native mold/Wild packages do not provide these aliases.
              linkerBins = pkgs.linkFarm "sigiro-${linker}-bin" [
                {
                  name = "${cc.targetPrefix}ld.bfd";
                  path = lib.getExe pkgs.${linker};
                }
                {
                  name = "ld.bfd";
                  path = lib.getExe pkgs.${linker};
                }
              ];
              driver = pkgs.writeShellScript "sigiro-${linker}-linker" ''
                exec ${cc}/bin/${cc.targetPrefix}cc -B${linkerBins}/ -fuse-ld=bfd "$@"
              '';
              targetUpper = lib.toUpper (builtins.replaceStrings [ "-" ] [ "_" ] staticTarget);
            in
            pkgs.mkShell {
              packages = devPackages;
              env = devEnv // {
                "CARGO_TARGET_${targetUpper}_LINKER" = "${driver}";
                CARGO_TARGET_DIR = "target/dev-${linker}";
              };
            };
          sigiro = rust.bundle {
            inherit dashboard docs;
            # `sigiro --version` names the commit.
            commit = self.shortRev or self.dirtyShortRev or null;
          };
        in
        {
          packages = {
            default = sigiro;
            inherit
              sigiro
              dashboard
              docs
              duckdb
              ;
            inherit (rust) openapi unbundled;
            cache-client = pkgs.cachix;
            rust-deps = rust.binArtifacts;
            rust-workspace-deps = rust.workspaceArtifacts;
            rust-wasm-deps = rust.wasmArtifacts;
          }
          // lib.optionalAttrs (system == "x86_64-linux") {
            dyndrv-dev = benchmark.dynamic;
            crane-dev = benchmark.crane;
            dyndrv-tool = benchmark.tool;
            dyndrv-repro-package-env = repros.package-env;
            dyndrv-repro-build-env = repros.build-env;
            dyndrv-repro-rustflags = repros.rustflags;
            dyndrv-repro-lib-bin = repros.lib-bin;
            dyndrv-repro-lib-bin-edited = repros.lib-bin-edited;
            dyndrv-repro-cargo-control = repros.cargo-control;
          }
          // lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
            image = pkgs.callPackage ./nix/image.nix { inherit sigiro version; };
          }
          // lib.optionalAttrs (lib.elem system aube.meta.platforms) {
            inherit aube;
          };

          checks =
            rust.checks
            // {
              inherit sigiro;
              portable = rust.portable sigiro;
            }
            // lib.optionalAttrs (system == "x86_64-linux") {
              dyndrv-dev-portable = rust.portable benchmark.dynamic;
              crane-dev-portable = rust.portable benchmark.crane;
            };

          devShells = lib.optionalAttrs (lib.elem system aube.meta.platforms) (
            {
              default = pkgs.mkShell {
                packages = devPackages;
                # Keep the normal target/ cache for the daily edit/build loop.
                env = devEnv;
              };
            }
            // lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
              mold = linkerShell "mold";
              wild = linkerShell "wild";
            }
          );

          formatter = pkgs.nixfmt;
        };

      bySystem = lib.genAttrs systems perSystem;
    in
    {
      packages = lib.mapAttrs (_: s: s.packages) bySystem;
      checks = lib.mapAttrs (_: s: s.checks) bySystem;
      devShells = lib.mapAttrs (_: s: s.devShells) bySystem;
      formatter = lib.mapAttrs (_: s: s.formatter) bySystem;
    };
}
