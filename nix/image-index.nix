# The multi-platform image: every Linux image in one OCI layout, under one
# `version` tag that resolves per platform. Each binary is compiled natively on
# its own system (normally fetched from the binary cache); building the images
# and the index only copies files, so one Linux host assembles every platform
# without QEMU.
#
# `publish` copies the layout to a registry; regctl uploads only the blobs the
# registry does not have yet.
{
  lib,
  runCommand,
  writeShellApplication,
  regclient,
  jq,
  version,
  # Docker architecture name -> single-platform image (nix/image.nix).
  images,
}:
let
  index =
    runCommand "sigiro-image-index-${version}"
      {
        nativeBuildInputs = [
          regclient
          jq
        ];
      }
      ''
        export HOME="$TMPDIR"
        ${lib.concatMapStrings (arch: ''
          regctl image import "ocidir://$out:${arch}" ${images.${arch}}
        '') (lib.attrNames images)}
        regctl index create "ocidir://$out:${version}" \
          --media-type application/vnd.docker.distribution.manifest.list.v2+json \
          ${lib.concatMapStringsSep " " (arch: "--ref ocidir://$out:${arch}") (lib.attrNames images)}
        # Leave one tag: the index. Its manifests stay, referenced by digest.
        ${lib.concatMapStrings (arch: ''
          regctl tag rm "ocidir://$out:${arch}"
        '') (lib.attrNames images)}
        # regctl stamps the build time on the layout's entry; drop it so the
        # layout is reproducible. The index itself carries no time.
        jq -c '.manifests[].annotations |= del(."org.opencontainers.image.created")' \
          "$out/index.json" > index.json
        mv index.json "$out/index.json"
        regctl manifest get "ocidir://$out:${version}"
      '';
in
{
  inherit index;

  publish = writeShellApplication {
    name = "sigiro-publish-image";
    runtimeInputs = [ regclient ];
    text = ''
      # Usage: sigiro-publish-image REGISTRY/REPO:TAG...
      # Log in first (regctl registry login, or docker login).
      if [ "$#" -eq 0 ]; then
        echo "usage: sigiro-publish-image REGISTRY/REPO:TAG..." >&2
        exit 2
      fi
      for ref in "$@"; do
        regctl image copy "ocidir://${index}:${version}" "$ref"
        echo "pushed $ref ($(regctl image digest "$ref"))"
      done
    '';
  };
}
