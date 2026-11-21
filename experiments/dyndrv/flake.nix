{
  description = "Rejected experiment: cargo-dyndrv versus Crane for sigiro";

  # Kept out of the root flake: it pins cargo-dyndrv and needs
  # `ca-derivations dynamic-derivations` with sandboxed `builder-rpc-v0`, which
  # CI and most machines do not have. See README.md.
  inputs = {
    sigiro.url = "path:../..";
    nixpkgs.follows = "sigiro/nixpkgs";
    cargo-dyndrv = {
      url = "github:obsidiansystems/cargo-dyndrv/d5245b85f82627fcaf041e2c57c8aeb7d57d20fe";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      sigiro,
      nixpkgs,
      cargo-dyndrv,
      ...
    }:
    let
      system = "x86_64-linux";
      inherit (nixpkgs) lib;
      inherit (sigiro.inputs) crane rust-overlay duckdb-static;

      pkgs = import nixpkgs {
        inherit system;
        overlays = [
          rust-overlay.overlays.default
          cargo-dyndrv.overlays.default
        ];
      };

      # Mirrors the root flake's build toolchain and Rust packaging.
      version = (lib.importTOML "${sigiro}/Cargo.toml").package.version;
      toolchainFile = (lib.importTOML "${sigiro}/rust-toolchain.toml").toolchain;
      buildToolchain = pkgs.rust-bin.stable.${toolchainFile.channel}.minimal.override {
        extensions = [
          "clippy"
          "rustfmt"
        ];
        targets = [
          pkgs.pkgsStatic.stdenv.hostPlatform.rust.rustcTarget
          "wasm32-unknown-unknown"
        ];
      };
      craneLib = (crane.mkLib pkgs).overrideToolchain (_: buildToolchain);
      rust = pkgs.callPackage "${sigiro}/nix/rust.nix" {
        inherit craneLib version;
        duckdb = duckdb-static.packages.${system}.default;
      };

      benchmark = pkgs.callPackage ./benchmark.nix {
        inherit
          rust
          craneLib
          buildToolchain
          version
          ;
      };
      repros = pkgs.callPackage ./repros.nix { inherit craneLib; };
    in
    {
      packages.${system} = {
        dyndrv-dev = benchmark.dynamic;
        crane-dev = benchmark.crane;
        dyndrv-tool = benchmark.tool;
        repro-package-env = repros.package-env;
        repro-build-env = repros.build-env;
        repro-rustflags = repros.rustflags;
        repro-lib-bin = repros.lib-bin;
        repro-lib-bin-edited = repros.lib-bin-edited;
        repro-cargo-control = repros.cargo-control;
      };

      checks.${system} = {
        dyndrv-dev-portable = rust.portable benchmark.dynamic;
        crane-dev-portable = rust.portable benchmark.crane;
      };
    };
}
