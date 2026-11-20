# The Rust side, built with crane: the sigiro binary and its checks.
#
# Everything targets nixpkgs' static platform (`pkgsStatic`): musl on Linux,
# so the binary is fully static and runs on any Linux kernel; on macOS, static
# libraries everywhere except the system's libSystem and libc++. DuckDB comes
# from the duckdb-static flake input, with all extensions compiled in.
#
# Dependency artifacts are built once (`cargoArtifacts`) and reused by every
# derivation that compiles the workspace, so a source edit recompiles only the
# sigiro crates.
{
  lib,
  stdenv,
  craneLib,
  pkgsStatic,
  duckdb,
  cacert,
  file,
  nukeReferences,
  darwin,
  version,
}:
let
  root = ../.;

  target = pkgsStatic.stdenv.hostPlatform.rust.rustcTarget;
  targetUpper = lib.toUpper (builtins.replaceStrings [ "-" ] [ "_" ] target);
  targetLower = builtins.replaceStrings [ "-" ] [ "_" ] target;
  cc = pkgsStatic.stdenv.cc;

  # Point cargo, cc-rs and the linker at the static toolchain for the target.
  # Build scripts still run on the build machine with its own compiler.
  targetEnv = {
    CARGO_BUILD_TARGET = target;
    "CARGO_TARGET_${targetUpper}_LINKER" = "${cc}/bin/${cc.targetPrefix}cc";
    "CC_${targetLower}" = "${cc}/bin/${cc.targetPrefix}cc";
    "CXX_${targetLower}" = "${cc}/bin/${cc.targetPrefix}c++";
    "AR_${targetLower}" = "${cc.bintools.bintools}/bin/${cc.targetPrefix}ar";
    # libduckdb-sys links duckdb-static's archive; nothing compiles DuckDB.
    DUCKDB_STATIC = "1";
    DUCKDB_LIB_DIR = "${duckdb}/lib";
    DUCKDB_INCLUDE_DIR = "${duckdb}/include";
  }
  // lib.optionalAttrs pkgsStatic.stdenv.hostPlatform.isLinux {
    # A plain static executable rather than Rust's default static-pie: the
    # static libstdc++ is not position-independent, so a PIE cannot link it.
    "CARGO_TARGET_${targetUpper}_RUSTFLAGS" = "-C relocation-model=static";
  };

  # What cargo reads. Not the frontend trees: dashboard/ and www/ reach the
  # binary as the separately built exports copied in by `bundle` below.
  src = lib.fileset.toSource {
    inherit root;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../build.rs
      ../clippy.toml
      ../.cargo
      ../src
      ../crates
      # Integration tests and the fixtures unit tests embed.
      (lib.fileset.fileFilter (f: f.hasExt "rs") ../tests)
      ../tests/fixtures
    ];
  };

  commonArgs = {
    inherit src version;
    pname = "sigiro";
    strictDeps = true;
    # Darwin's Rust build scripts use the same target triple as the application.
    # Both need the static iconv archive, which lives in its dev output.
    buildInputs = lib.optionals stdenv.hostPlatform.isDarwin [
      (lib.getDev pkgsStatic.libiconv)
    ];
    env = targetEnv;
  };

  # The binary target only: what `nix build` needs.
  binArgs = commonArgs // {
    cargoExtraArgs = "--locked --package sigiro --bin sigiro";
    doCheck = false;
    nativeBuildInputs = [
      nukeReferences
    ]
    ++ lib.optionals stdenv.hostPlatform.isDarwin [
      darwin.sigtool
    ];
    # Scrubbing cannot make a dynamic dependency portable: it corrupts the
    # loader path. Reject those paths before touching Mach-O debug references.
    postFixup =
      lib.optionalString stdenv.hostPlatform.isDarwin ''
        otool -L "$out/bin/sigiro" > runtime-libraries.txt
        if tail -n +2 runtime-libraries.txt | grep -q '/nix/store/'; then
          cat runtime-libraries.txt >&2
          echo "sigiro still links a Nix runtime library" >&2
          exit 1
        fi
      ''
      + ''
        nuke-refs "$out/bin/sigiro"
      ''
      + lib.optionalString stdenv.hostPlatform.isDarwin ''
        codesign --force --sign - "$out/bin/sigiro"
      '';
    # Cargo.toml keeps line tables (`debug = "line-tables-only"`,
    # `strip = false`) for in-process symbolization of self-profiles. Nix's
    # default `strip -S` would remove them.
    dontStrip = true;
  };
  binArtifacts = craneLib.buildDepsOnly binArgs;

  # The whole workspace, all targets: what clippy and the tests need. Except
  # crates/segments-wasm, a WebAssembly-only cdylib (`wasmArgs`).
  workspaceArgs = commonArgs // {
    cargoExtraArgs = "--locked --workspace --exclude sigiro-segments-wasm";
  };
  workspaceArtifacts = craneLib.buildDepsOnly workspaceArgs;

  # crates/segments-wasm, for the target it exists for (cloud/ loads it).
  wasmArgs = {
    inherit src version;
    pname = "sigiro-segments-wasm";
    strictDeps = true;
    cargoExtraArgs = "--locked --package sigiro-segments-wasm";
    CARGO_BUILD_TARGET = "wasm32-unknown-unknown";
    doCheck = false;
  };

  wasmArtifacts = craneLib.buildDepsOnly wasmArgs;

  # sigiro without the dashboard or docs. Used to print the OpenAPI spec the
  # docs render, which breaks the cycle "binary embeds docs, docs describe the
  # binary's API".
  unbundled = craneLib.buildPackage (
    binArgs
    // {
      pname = "sigiro-unbundled";
      cargoArtifacts = binArtifacts;
    }
  );

  bundle =
    {
      dashboard,
      docs,
      commit ? null,
    }:
    craneLib.buildPackage (
      binArgs
      // {
        cargoArtifacts = binArtifacts;
        preBuild = ''
          mkdir -p dashboard/dist www
          cp -r ${dashboard} dashboard/dist/dashboard
          cp -r ${docs}/dist-binary www/dist-binary
          cp -r ${docs}/dist-mcp www/dist-mcp
          chmod -R u+w dashboard www
        '';
        env =
          binArgs.env
          // {
            # build.rs fails rather than ship a binary without its UI or manual.
            SIGIRO_REQUIRE_DASHBOARD = "1";
            SIGIRO_REQUIRE_DOCS = "1";
          }
          // lib.optionalAttrs (commit != null) { SIGIRO_BUILD_COMMIT = commit; };

        # A portable binary must not need anything from the store at run
        # time. Nix records a runtime dependency for every store path found in
        # the output; there must be none.
        allowedReferences = [ ];

        meta = {
          description = "Single-binary OpenTelemetry backend: traces, logs, metrics and profiles over OTLP";
          homepage = "https://sigiro.com";
          license = {
            fullName = "Functional Source License, Version 1.1, ALv2 Future License";
            url = "https://fsl.software/FSL-1.1-ALv2.template.md";
            free = false;
            redistributable = true;
          };
          mainProgram = "sigiro";
          platforms = lib.platforms.linux ++ lib.platforms.darwin;
        };
      }
    );
in
{
  inherit
    src
    unbundled
    bundle
    binArtifacts
    workspaceArtifacts
    wasmArtifacts
    target
    targetEnv
    ;

  openapi = stdenv.mkDerivation {
    pname = "sigiro-openapi";
    inherit version;
    dontUnpack = true;
    nativeBuildInputs = [ unbundled ];
    buildPhase = ''
      mkdir -p "$out"
      sigiro openapi > "$out/openapi.json"
      test -s "$out/openapi.json"
    '';
    dontInstall = true;
  };

  # Fails unless `sigiro` links nothing but the operating system.
  portable =
    sigiro:
    stdenv.mkDerivation {
      name = "sigiro-portable";
      dontUnpack = true;
      nativeBuildInputs = [ file ];
      buildPhase = ''
        bin=${sigiro}/bin/sigiro
        file -L "$bin"
        ${
          if stdenv.hostPlatform.isLinux then
            ''
              file -L "$bin" | grep -q 'static-pie linked\|statically linked' || {
                echo "sigiro is dynamically linked" >&2; exit 1; }
            ''
          else
            ''
              /usr/bin/otool -L "$bin" | tail -n +2 | awk '{print $1}' | while read -r lib; do
                case "$lib" in
                  /usr/lib/*|/System/Library/*) ;;
                  *) echo "sigiro links $lib, which is not part of macOS" >&2; exit 1 ;;
                esac
              done
            ''
        }
        "$bin" --version
        touch "$out"
      '';
      dontInstall = true;
    };

  checks = {
    clippy = craneLib.cargoClippy (
      workspaceArgs
      // {
        cargoArtifacts = workspaceArtifacts;
        cargoClippyExtraArgs = "--all-targets --all-features -- -D warnings";
      }
    );

    clippy-wasm = craneLib.cargoClippy (
      wasmArgs
      // {
        cargoArtifacts = wasmArtifacts;
        cargoClippyExtraArgs = "--all-targets -- -D warnings";
      }
    );

    fmt = craneLib.cargoFmt { inherit src; };

    test = craneLib.cargoTest (
      workspaceArgs
      // {
        cargoArtifacts = workspaceArtifacts;
        # reqwest's rustls client refuses to build without a CA store, which
        # the sandbox lacks; the tests talk to loopback servers only.
        nativeCheckInputs = [ cacert ];
      }
    );
  };
}
