# The container image (ghcr.io/akshatagarwl/sigiro): the static binary and
# nothing else. It keeps the contract of the distroless image it replaced: the
# binary at /usr/local/bin/sigiro, data in /var/lib/sigiro, user nonroot
# (65532), the shell-free healthcheck, and `serve` by default.
#
# Built with nix2container: the result is a small JSON description of the
# image, not a tarball. Layers are produced only when the image is copied to
# Docker or a registry, and a registry is sent only the layers it lacks.
#
# `sigiro` may belong to another Linux system than the one building the image:
# the image only copies the static binary, so `architecture` names the binary's
# architecture and no emulator is needed.
{
  runCommand,
  nix2container,
  sigiro,
  version,
  # Docker's name for the binary's architecture (amd64, arm64).
  architecture,
}:
let
  seconds = s: s * 1000000000;
  # Copied, not referenced: the image holds one file, not a store closure.
  root = runCommand "sigiro-image-root-${architecture}" { } ''
    mkdir -p $out/usr/local/bin $out/var/lib/sigiro $out/tmp $out/etc
    cp ${sigiro}/bin/sigiro $out/usr/local/bin/sigiro
    printf '%s\n' 'root:x:0:0:root:/root:/sbin/nologin' \
      'nonroot:x:65532:65532:nonroot:/home/nonroot:/sbin/nologin' > $out/etc/passwd
    printf '%s\n' 'root:x:0:' 'nonroot:x:65532:' > $out/etc/group
  '';
in
nix2container.buildImage {
  name = "ghcr.io/akshatagarwl/sigiro";
  # One tag per platform. The published `version` tag is a multi-platform
  # index over these (nix/publish-image.nix).
  tag = "${version}-${architecture}";
  arch = architecture;
  copyToRoot = root;
  perms = [
    {
      path = root;
      regex = "/var/lib/sigiro$";
      mode = "0755";
      uid = 65532;
      gid = 65532;
      uname = "nonroot";
      gname = "nonroot";
    }
    {
      path = root;
      regex = "/tmp$";
      mode = "1777";
    }
  ];

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
    # deployment health checks must use this exec form too. Upstream
    # nix2container drops this field; flake.nix pins a fork that keeps it.
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
}
