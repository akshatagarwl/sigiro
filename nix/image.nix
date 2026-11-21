# The container image (ghcr.io/akshatagarwl/sigiro): the static binary and
# nothing else. It keeps the contract of the distroless image it replaces: the
# binary at /usr/local/bin/sigiro, data in /var/lib/sigiro, user nonroot
# (65532), the shell-free healthcheck, and `serve` by default.
#
# `sigiro` may belong to another Linux system than the one building the image:
# assembling it only copies the static binary, so `architecture` names the
# binary's architecture and no emulator is needed (nix/image-index.nix).
{
  lib,
  stdenv,
  dockerTools,
  sigiro,
  version,
  # Docker's name for the binary's architecture (amd64, arm64).
  architecture ? stdenv.hostPlatform.go.GOARCH,
}:
let
  seconds = s: s * 1000000000;
in
dockerTools.buildLayeredImage {
  name = "ghcr.io/akshatagarwl/sigiro";
  tag = version;
  inherit architecture;
  # Copied, not referenced: the image holds one file, not a store closure.
  extraCommands = ''
    mkdir -p usr/local/bin var/lib/sigiro tmp etc
    cp ${sigiro}/bin/sigiro usr/local/bin/sigiro
    chmod 1777 tmp
    echo 'root:x:0:0:root:/root:/sbin/nologin' > etc/passwd
    echo 'nonroot:x:65532:65532:nonroot:/home/nonroot:/sbin/nologin' >> etc/passwd
    echo 'root:x:0:' > etc/group
    echo 'nonroot:x:65532:' >> etc/group
  '';
  fakeRootCommands = ''
    chown 65532:65532 var/lib/sigiro
  '';

  config = {
    Entrypoint = [ "/usr/local/bin/sigiro" ];
    Cmd = [ "serve" ];
    User = "65532:65532";
    WorkingDir = "/var/lib/sigiro";
    Env = [ "SIGIRO_DATA_DIR=/var/lib/sigiro" ];
    ExposedPorts = {
      "4317/tcp" = { };
      "4318/tcp" = { };
      "9999/tcp" = { };
    };
    # `sigiro healthcheck` exits 0 only when ready. The image has no shell, so
    # deployment health checks must use this exec form too.
    Healthcheck = {
      Test = [
        "CMD"
        "/usr/local/bin/sigiro"
        "healthcheck"
      ];
      Interval = seconds 10;
      Timeout = seconds 5;
      StartPeriod = seconds 120;
      Retries = 6;
    };
  };

  meta.platforms = lib.platforms.linux;
}
