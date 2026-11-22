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
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    # The container image builder (nix/image.nix). A fork of
    # nlewo/nix2container carrying nlewo/nix2container#198, which keeps the
    # image's Healthcheck; return to upstream once that merges.
    nix2container = {
      url = "github:akshatagarwl/nix2container/healthcheck";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      duckdb-static,
      crane,
      rust-overlay,
      nix2container,
    }:
    let
      inherit (nixpkgs) lib;

      # Native release hosts supported by the pinned nixpkgs (.github/workflows/ci.yml).
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];

      # Container platforms: Docker architecture name -> the system whose
      # static binary it ships.
      imageSystems = {
        amd64 = "x86_64-linux";
        arm64 = "aarch64-linux";
      };

      cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
      version = cargoToml.package.version;
      toolchainFile = (builtins.fromTOML (builtins.readFile ./rust-toolchain.toml)).toolchain;

      perSystem =
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
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

          rust = pkgs.callPackage ./nix/rust.nix { inherit craneLib duckdb version; };
          dashboard = pkgs.callPackage ./nix/dashboard.nix { inherit version; };
          docs = pkgs.callPackage ./nix/docs.nix {
            inherit version;
            inherit (rust) openapi;
          };
          javascriptPackages = [
            pkgs.nodejs_24
            pkgs.pnpm_10
            pkgs.git
            pkgs.gh
          ];
          devPackages = javascriptPackages ++ [
            devToolchain
            # Must equal the locked wasm-bindgen crate; build-segments-wasm.sh checks it.
            pkgs.wasm-bindgen-cli
            # Deployment-contract tests.
            pkgs.python3
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
              # Unwrapped: the native wrapper assumes a glibc dynamic link and
              # adds glibc's loader, which aarch64 GCC's `-Bstatic` does not stop.
              linkerExe = lib.getExe pkgs."${linker}-unwrapped";
              linkerBins = pkgs.linkFarm "sigiro-${linker}-bin" [
                {
                  name = "${cc.targetPrefix}ld.bfd";
                  path = linkerExe;
                }
                {
                  name = "ld.bfd";
                  path = linkerExe;
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
          # One image per Linux system, all assembled by this system's tools
          # from the binaries the others build.
          containerTools = nix2container.packages.${system};
          linuxImages = lib.mapAttrs (
            architecture: imageSystem:
            pkgs.callPackage ./nix/image.nix {
              inherit version architecture;
              inherit (containerTools) nix2container;
              inherit (bySystem.${imageSystem}.packages) sigiro;
            }
          ) imageSystems;
          hostArchitecture = pkgs.stdenv.hostPlatform.go.GOARCH;
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
          // lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
            # This system's image: `nix run .#image.copyToDockerDaemon`.
            image = linuxImages.${hostArchitecture};
            # Every Linux platform under one tag (nix/publish-image.nix).
            publish-image = pkgs.callPackage ./nix/publish-image.nix {
              skopeo = containerTools.skopeo-nix2container;
              images = linuxImages;
            };
          };

          checks =
            rust.checks
            // {
              inherit sigiro;
              portable = rust.portable sigiro;
            }
            // lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
              image = pkgs.callPackage ./nix/image-check.nix {
                skopeo = containerTools.skopeo-nix2container;
                image = linuxImages.${hostArchitecture};
                architecture = hostArchitecture;
                inherit sigiro;
              };
            };

          devShells = {
            javascript = pkgs.mkShell {
              packages = javascriptPackages;
            };
            default = pkgs.mkShell {
              packages = devPackages;
              # Keep the normal target/ cache for the daily edit/build loop.
              env = devEnv;
            };
          }
          // lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
            mold = linkerShell "mold";
            wild = linkerShell "wild";
          };

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
