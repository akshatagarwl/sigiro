# Checks the image as a registry would receive it: copies it to an OCI layout
# with the same skopeo that publishes it, then checks its config and its one
# layer. Runs in the sandbox, with no Docker and no network.
{
  runCommand,
  skopeo,
  jq,
  image,
  sigiro,
  architecture,
}:
runCommand "sigiro-image-check-${architecture}"
  {
    nativeBuildInputs = [
      skopeo
      jq
    ];
  }
  ''
    export HOME="$TMPDIR"
    skopeo --insecure-policy --tmpdir "$TMPDIR" copy nix:${image} "oci:$TMPDIR/oci:image"
    blob() { echo "$TMPDIR/oci/blobs/''${1/://}"; }
    manifest="$(blob "$(jq -r '.manifests[0].digest' "$TMPDIR/oci/index.json")")"
    config="$(blob "$(jq -r .config.digest "$manifest")")"
    jq . "$config"

    jq -e --arg arch ${architecture} '
      .os == "linux"
      and .architecture == $arch
      and .config.Entrypoint == ["/usr/local/bin/sigiro"]
      and .config.Cmd == ["serve"]
      and .config.User == "65532:65532"
      and .config.WorkingDir == "/var/lib/sigiro"
      and .config.Env == ["SIGIRO_DATA_DIR=/var/lib/sigiro"]
      and (.config.ExposedPorts | keys) == ["4317/tcp", "4318/tcp", "9999/tcp"]
      and .config.Healthcheck.Test == ["CMD", "/usr/local/bin/sigiro", "healthcheck"]
    ' "$config" > /dev/null || { echo "image config is wrong" >&2; exit 1; }

    [ "$(jq '.layers | length' "$manifest")" -eq 1 ] || { echo "expected one layer" >&2; exit 1; }
    layer="$(blob "$(jq -r '.layers[0].digest' "$manifest")")"
    tar --numeric-owner -tvf "$layer" | tee listing
    ! grep -q 'nix/store' listing || { echo "the image holds store paths" >&2; exit 1; }
    grep -Eq '^drwxr-xr-x 65532/65532 .* /var/lib/sigiro/?$' listing \
      || { echo "/var/lib/sigiro must belong to nonroot" >&2; exit 1; }
    grep -Eq '^drwxrwxrwt .* /tmp/?$' listing || { echo "/tmp must be 1777" >&2; exit 1; }
    mkdir root
    tar -xf "$layer" -C root
    cmp root/usr/local/bin/sigiro ${sigiro}/bin/sigiro
    touch $out
  ''
