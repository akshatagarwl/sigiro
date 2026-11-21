# Publishes the multi-platform image. Each platform's image is pushed under
# `<ref>-<arch>`, then `<ref>` becomes an index over them, so one tag resolves
# per platform. skopeo builds each layer from the store while pushing and skips
# layers the registry already has; nothing is written to the store first.
#
# Every image only copies a static binary that was compiled natively on its own
# system (normally fetched from the binary cache), so one Linux host publishes
# every platform without QEMU.
{
  lib,
  writeShellApplication,
  regclient,
  skopeo,
  # Docker architecture name -> single-platform image (nix/image.nix).
  images,
}:
writeShellApplication {
  name = "sigiro-publish-image";
  runtimeInputs = [
    regclient
    skopeo
  ];
  text = ''
    # Usage: sigiro-publish-image REGISTRY/REPO:TAG...
    # Log in first with `docker login`: skopeo and regctl both read its
    # credentials.
    if [ "$#" -eq 0 ]; then
      echo "usage: sigiro-publish-image REGISTRY/REPO:TAG..." >&2
      exit 2
    fi
    for ref in "$@"; do
      case "''${ref##*/}" in
        *:*) ;;
        *)
          echo "sigiro-publish-image: $ref has no tag" >&2
          exit 2
          ;;
      esac
    done

    first="$1"
    platforms=()
    ${lib.concatStrings (
      lib.mapAttrsToList (arch: image: ''
        skopeo --insecure-policy copy nix:${image} "docker://$first-${arch}"
        platforms+=(--ref "$first-${arch}")
      '') images
    )}
    regctl index create "$first" "''${platforms[@]}"
    for ref in "''${@:2}"; do
      regctl image copy "$first" "$ref"
    done
    for ref in "$@"; do
      echo "pushed $ref ($(regctl image digest "$ref"))"
    done
  '';
}
