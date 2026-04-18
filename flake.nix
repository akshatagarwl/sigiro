{
  description = "Sigiro — observability pipeline with DuckDB/DuckLake storage";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    crane = {
      url = "github:ipetkov/crane";
    };
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, crane, fenix }:
    let
      # nixpkgs pkgsCross attribute name for each Rust target triple.
      crossTargetPkgs = {
        "aarch64-unknown-linux-gnu" = "aarch64-multiplatform";
        "x86_64-unknown-linux-gnu"  = "gnu64";
      };

      # Cross-targets available per build system.
      # Empty list = native only.
      crossTargetsFor = buildSystem: {
        "aarch64-darwin" = [
          "aarch64-unknown-linux-gnu"
          "x86_64-unknown-linux-gnu"
        ];
        "x86_64-darwin"  = [ "x86_64-unknown-linux-gnu" ];
        "x86_64-linux"   = [ ];
      }.${buildSystem} or [ ];

    in flake-utils.lib.eachSystem [
      "aarch64-darwin"
      "x86_64-darwin"
      "x86_64-linux"
    ] (buildSystem: let

      pkgs = import nixpkgs {
        system = buildSystem;
        config.allowUnsupportedSystem = true;
      };
      lib = pkgs.lib;

      crossTargets = crossTargetsFor buildSystem;

      # Fenix toolchain for the native build.
      nativeToolchain = with fenix.packages.${buildSystem}; stable.toolchain;

      craneLib = crane.mkLib pkgs;
      src = craneLib.cleanCargoSource ./.;

      nativeBuildInputs = with pkgs; [ cmake pkg-config python3 ];

      # Dependency derivation (DuckDB C++ compiles here).
      commonDeps = craneLib.buildDepsOnly {
        inherit src;
        strictDeps = true;
        inherit nativeBuildInputs;
        preBuild = "ulimit -n 65536 2>/dev/null || true";
      };

      # ---- Native package ----
      native = craneLib.buildPackage {
        inherit src;
        strictDeps = true;
        inherit nativeBuildInputs;
        cargoArtifacts = commonDeps;
        preBuild = "ulimit -n 65536 2>/dev/null || true";
        preCheck = "export HOME=$(mktemp -d)";
      };

      # ---- Cross-compiled packages ----
      crossPackages = lib.genAttrs crossTargets (crossTarget: let
        crossPkgs = pkgs.pkgsCross.${crossTargetPkgs.${crossTarget}};

        fenixToolchain = with fenix.packages.${buildSystem};
          combine [ minimal.cargo minimal.rustc targets.${crossTarget}.latest.rust-std ];

        crossEnvBase = {
          "CARGO_TARGET_${lib.replaceStrings ["-"] ["_"] (lib.toUpper crossTarget)}_LINKER"
            = "${crossPkgs.stdenv.cc.targetPrefix}gcc";
          "CC_${lib.replaceStrings ["-"] ["_"] (lib.toUpper crossTarget)}"
            = "${crossPkgs.stdenv.cc.targetPrefix}gcc";
          "CXX_${lib.replaceStrings ["-"] ["_"] (lib.toUpper crossTarget)}"
            = "${crossPkgs.stdenv.cc.targetPrefix}g++";
          "AR_${lib.replaceStrings ["-"] ["_"] (lib.toUpper crossTarget)}"
            = "${crossPkgs.stdenv.cc.targetPrefix}ar";
          PKG_CONFIG_ALLOW_CROSS = "1";
        };

        crossDeps = (craneLib.overrideToolchain fenixToolchain).buildDepsOnly {
          inherit src;
          strictDeps = true;
          nativeBuildInputs = nativeBuildInputs ++ [
            crossPkgs.stdenv.cc
            crossPkgs.stdenv.cc.bintools
          ];
          env = crossEnvBase // {
            "CFLAGS_${lib.replaceStrings ["-"] ["_"] (lib.toUpper crossTarget)}"
              = "-isystem ${crossPkgs.glibc.dev}/include";
            "CXXFLAGS_${lib.replaceStrings ["-"] ["_"] (lib.toUpper crossTarget)}"
              = "-isystem ${crossPkgs.glibc.dev}/include -std=c++17";
          };
          preBuild = "ulimit -n 65536 2>/dev/null || true";
        };

        crossEnv = crossEnvBase // {
          "CFLAGS_${lib.replaceStrings ["-"] ["_"] (lib.toUpper crossTarget)}"
            = "-isystem ${crossPkgs.glibc.dev}/include";
          "CXXFLAGS_${lib.replaceStrings ["-"] ["_"] (lib.toUpper crossTarget)}"
            = "-isystem ${crossPkgs.glibc.dev}/include -std=c++17";
        };

      in (craneLib.overrideToolchain fenixToolchain).buildPackage {
        inherit src;
        strictDeps = true;
        nativeBuildInputs = nativeBuildInputs ++ [
          crossPkgs.stdenv.cc
          crossPkgs.stdenv.cc.bintools
        ];
        cargoArtifacts = crossDeps;
        cargoExtraArgs = "--target ${crossTarget}";
        env = crossEnv;
        preBuild = "ulimit -n 65536 2>/dev/null || true";
        doCheck = false;
        installPhase = ''
          mkdir -p $out/bin
          cp target/${crossTarget}/release/sigiro $out/bin/sigiro
        '';
        postFixup = ''
          echo "--- Cross-compiled binary info ---"
          ${pkgs.buildPackages.file}/bin/file $out/bin/sigiro
          ${pkgs.buildPackages.binutils}/bin/${crossPkgs.stdenv.cc.targetPrefix}readelf -d $out/bin/sigiro 2>/dev/null || true
        '';
      });

    in {
      packages =
        { nativeBuild = native; }
        // builtins.listToAttrs (
          map (name: { name = "sigiro-${name}"; value = crossPackages.${name}; })
          (builtins.attrNames crossPackages)
        );

      devShells.default = pkgs.mkShell {
        packages = with pkgs; [
          nativeToolchain
          cmake
          pkg-config
          python3
          cargo-watch
          cargo-outdated
          rust-analyzer
        ];
        shellHook = ''
          echo "sigiro dev shell"
          echo "  rustc: $(rustc --version)"
          echo ""
          echo "  cargo build          — native build"
          echo "  nix build            — native Nix build"
          ${lib.optionalString (crossTargets != [ ]) ''
            echo "  nix build .#sigiro-<target>  — cross-compile:"
            ${lib.concatMapStringsSep "\n" (t: "            echo '    ${t}'") crossTargets}
          ''}
        '';
      };
    });
}
