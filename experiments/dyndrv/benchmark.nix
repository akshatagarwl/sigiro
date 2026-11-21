{
  lib,
  stdenv,
  runCommand,
  writeShellScriptBin,
  writeExtern,
  cargo-dyndrv,
  coreutils,
  bash,
  gnumake,
  cmake,
  perl,
  python3,
  pkg-config,
  gnused,
  gnugrep,
  gawk,
  findutils,
  diffutils,
  nukeReferences,
  rust,
  craneLib,
  buildToolchain,
  version,
}:
let
  common = {
    inherit (rust) src;
    inherit version;
    pname = "sigiro-benchmark-dev";
    env = rust.targetEnv // {
      CARGO_PROFILE = "dev";
    };
    cargoExtraArgs = "--locked --package sigiro --bin sigiro";
    doCheck = false;
    dontStrip = true;
  };
  compiler = writeShellScriptBin "rustc" ''
    flags=(-C split-debuginfo=unpacked)
    case "''${CARGO_PKG_NAME-}" in
      ""|sigiro|sigiro-core|sigiro-segments) ;;
      *) flags+=(--cap-lints allow) ;;
    esac
    case " $* " in
      *"--target=${rust.target}"*|*"--target ${rust.target}"*)
        flags+=(-C relocation-model=static)
        ;;
    esac
    exec ${buildToolchain}/bin/rustc "$@" "''${flags[@]}"
  '';
  metadataEnvFix = ''
    env.insert("CARGO_PKG_NAME".into(), meta.name.to_string().into());
    env.insert("CARGO_PKG_AUTHORS".into(), meta.authors.join(":").into());
    env.insert("CARGO_PKG_DESCRIPTION".into(), meta.description.clone().unwrap_or_default().into());
    env.insert("CARGO_PKG_HOMEPAGE".into(), meta.homepage.clone().unwrap_or_default().into());
    env.insert("CARGO_PKG_REPOSITORY".into(), meta.repository.clone().unwrap_or_default().into());
    env.insert("CARGO_PKG_LICENSE".into(), meta.license.clone().unwrap_or_default().into());
    env.insert("CARGO_PKG_LICENSE_FILE".into(), meta.license_file.as_ref().map(ToString::to_string).unwrap_or_default().into());
    env.insert("CARGO_PKG_README".into(), meta.readme.as_ref().map(ToString::to_string).unwrap_or_default().into());
    env.insert("CARGO_PKG_RUST_VERSION".into(), meta.rust_version.as_ref().map(ToString::to_string).unwrap_or_default().into());
    env.insert("CARGO_MANIFEST_LINKS".into(), meta.links.clone().unwrap_or_default().into());
  '';
  unitEnvFix = ''
    let mut env = base_env.clone();
    env.insert("CARGO_CRATE_NAME".into(), unit.target.name.replace("-", "_").into());
    if unit.target.kind.iter().any(|kind| kind == "bin") {
        env.insert("CARGO_BIN_NAME".into(), unit.target.name.clone().into());
    }
  '';
  scriptEnvFix = ''
    env.insert("OPT_LEVEL".into(), unit.profile.opt_level.clone().into());
    env.insert("DEBUG".into(), (unit.profile.debuginfo > 0).to_string().into());
    env.insert("PROFILE".into(), if unit.profile.name == "dev" { "debug" } else { &unit.profile.name }.to_owned().into());
    env.insert("NUM_JOBS".into(), std::env::var("NIX_BUILD_CORES").unwrap_or_else(|_| "1".to_owned()).into());
    env.insert("CARGO_ENCODED_RUSTFLAGS".into(), std::env::var("CARGO_ENCODED_RUSTFLAGS").unwrap_or_default().into());
  '';
  tool = cargo-dyndrv.overrideAttrs (old: {
    postPatch = (old.postPatch or "") + ''
      substituteInPlace cargo-dyndrv/src/main.rs \
        --replace-fail '    // TODO: more of these' ${lib.escapeShellArg metadataEnvFix} \
        --replace-fail 'let mut env = base_env.clone();' ${lib.escapeShellArg unitEnvFix} \
        --replace-fail 'env.insert("OPT_LEVEL".into(), unit.profile.opt_level.clone().into());' ${lib.escapeShellArg scriptEnvFix}
    '';
  });
  nativeTools = [
    coreutils
    bash
    gnumake
    cmake
    perl
    python3
    pkg-config
    gnused
    gnugrep
    gawk
    findutils
    diffutils
  ];
  nativeCrates = lib.filter (
    package:
    lib.elem package.name [
      "aws-lc-sys"
      "tikv-jemalloc-sys"
      "libduckdb-sys"
      "libsqlite3-sys"
      "zstd-sys"
      "lz4-sys"
      "bzip2-sys"
      "libz-sys"
      "ring"
      "backtrace"
      "psm"
    ]
  ) (builtins.fromTOML (builtins.readFile "${rust.src}/Cargo.lock")).package;
  extern = writeExtern (
    lib.listToAttrs (
      map (package: {
        name = "registry+https://github.com/rust-lang/crates.io-index#${package.name}@${package.version}";
        value = {
          path = map (nativeTool: "${lib.getBin nativeTool}/bin") nativeTools;
          env = rust.targetEnv // {
            NUM_JOBS = "8";
            PROFILE = "debug";
            DEBUG = "false";
          };
        };
      }) nativeCrates
    )
  );
  vendor = craneLib.vendorCargoDeps { inherit (rust) src; };
  materializedVendor = runCommand "dyndrv-benchmark-vendor" { } ''
    mkdir -p "$out"
    cp -rL ${vendor}/. "$out/"
    chmod u+w "$out/config.toml"
    substituteInPlace "$out/config.toml" --replace-fail "${vendor}" "$out"
  '';
  graph = craneLib.mkCargoDerivation (
    common
    // {
      pname = "dyndrv-benchmark-graph";
      cargoArtifacts = null;
      cargoVendorDir = materializedVendor;
      doInstallCargoArtifacts = false;
      nativeBuildInputs = [ tool ];
      outputs = [ "sigiro.drv" ];
      out = "/nonexistent";
      # Mutable Cargo state must not become part of imported crate-source NARs.
      preUnpack = ''
        export CARGO_HOME="$TMPDIR/dyndrv-cargo-home"
        export CARGO_TARGET_DIR="$TMPDIR/dyndrv-target"
      '';
      requiredSystemFeatures = [ "builder-rpc-v0" ];
      __contentAddressed = true;
      outputHashMode = "text";
      outputHashAlgo = "sha256";
      env = common.env // {
        CARGO = "${buildToolchain}/bin/cargo";
        RUSTC = "${compiler}/bin/rustc";
        CC = rust.targetEnv."CC_${builtins.replaceStrings [ "-" ] [ "_" ] rust.target}";
        CXX = rust.targetEnv."CXX_${builtins.replaceStrings [ "-" ] [ "_" ] rust.target}";
        HOST_CC = "${stdenv.cc}/bin/${stdenv.cc.targetPrefix}cc";
        HOST_CXX = "${stdenv.cc}/bin/${stdenv.cc.targetPrefix}c++";
        EXTERN_PATH = extern;
        CARGO_DYNDRV_IN_DRV = "";
        __CARGO_TEST_CHANNEL_OVERRIDE_DO_NOT_USE_THIS = "nightly";
      };
      # stdenv setup selects native CC; graph generation needs the musl target tools.
      preBuild = ''
        export CC=${
          lib.escapeShellArg rust.targetEnv."CC_${builtins.replaceStrings [ "-" ] [ "_" ] rust.target}"
        }
        export CXX=${
          lib.escapeShellArg rust.targetEnv."CXX_${builtins.replaceStrings [ "-" ] [ "_" ] rust.target}"
        }
        printf 'Target CC: %s\nTarget CXX: %s\n' "$CC" "$CXX"
      '';
      buildPhaseCargoCommand = ''
        ${lib.getExe tool} build --offline --locked \
          --target ${rust.target} --package sigiro --bin sigiro
      '';
      installPhaseCommand = ":";
      dontFixup = true;
    }
  );
in
{
  inherit tool;
  dynamic =
    runCommand "sigiro-dyndrv-dev-${version}"
      {
        nativeBuildInputs = [ nukeReferences ];
        allowedReferences = [ ];
      }
      ''
        mkdir -p "$out/bin"
        cp ${builtins.outputOf graph."sigiro.drv".outPath "out"}/sigiro "$out/bin/sigiro"
        nuke-refs "$out/bin/sigiro"
      '';
  crane = craneLib.buildPackage (
    common
    // {
      cargoArtifacts = craneLib.buildDepsOnly (
        common
        // {
          buildPhaseCargoCommand = "cargoWithProfile build ${common.cargoExtraArgs}";
        }
      );
      nativeBuildInputs = [ nukeReferences ];
      postFixup = ''nuke-refs "$out/bin/sigiro"'';
      allowedReferences = [ ];
    }
  );
}
