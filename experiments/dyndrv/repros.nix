{
  lib,
  runCommand,
  buildDynamicCrate,
  craneLib,
}:
let
  src = lib.fileset.toSource {
    root = ./repro-crate;
    fileset = ./repro-crate;
  };
  editedSrc = runCommand "source" { } ''
    cp -R ${src} "$out"
    chmod -R u+w "$out"
    substituteInPlace "$out/src/main.rs" --replace-fail 'baseline:' 'edited:'
  '';
  dynamic =
    features: source:
    buildDynamicCrate {
      pname = "dyndrv-repro";
      version = "0.1.0";
      src = source;
      outputs = [ "dyndrv-repro" ];
      cargoLock.lockFile = ./repro-crate/Cargo.lock;
      cargoBuildType = "debug";
      cargoBuildFlags = [
        "--bin"
        "dyndrv-repro"
      ]
      ++ lib.optionals (features != "") [
        "--features"
        features
      ];
      env.__CARGO_TEST_CHANNEL_OVERRIDE_DO_NOT_USE_THIS = "nightly";
      env.RUSTFLAGS = "--cfg required_by_repro";
    };
in
{
  package-env = dynamic "package-env" src;
  build-env = dynamic "build-env" src;
  rustflags = dynamic "rustflags" src;
  lib-bin = dynamic "" src;
  lib-bin-edited = dynamic "" editedSrc;
  cargo-control = craneLib.buildPackage {
    pname = "dyndrv-repro-control";
    version = "0.1.0";
    inherit src;
    env.CARGO_PROFILE = "dev";
    env.RUSTFLAGS = "--cfg required_by_repro";
    cargoExtraArgs = "--locked --features package-env,build-env,rustflags -vv";
    doCheck = false;
  };
}
