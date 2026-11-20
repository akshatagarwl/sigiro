# aube, the repository's Node package manager, for the dev shell. The version,
# URLs and sha256s come from mise.lock, so mise and Nix install the same
# artifact. Linux uses the static musl build (no ELF patching needed).
{
  lib,
  stdenvNoCC,
  fetchurl,
}:
let
  lock = builtins.fromTOML (builtins.readFile ../mise.lock);
  key = "packslip:github.com/aubepkg/aube";
  tool = lib.head lock.tools.${key};
  platform =
    {
      x86_64-linux = "linux-x64-musl";
      aarch64-linux = "linux-arm64-musl";
      aarch64-darwin = "macos-arm64";
    }
    .${stdenvNoCC.hostPlatform.system}
      or (throw "aube: mise.lock pins no artifact for ${stdenvNoCC.hostPlatform.system}");
  artifact = tool."platforms.${platform}";
in
stdenvNoCC.mkDerivation {
  pname = "aube";
  inherit (tool) version;

  src = fetchurl {
    inherit (artifact) url;
    sha256 = lib.removePrefix "sha256:" artifact.checksum;
  };
  sourceRoot = ".";

  dontConfigure = true;
  dontBuild = true;

  installPhase = ''
    runHook preInstall
    install -Dm755 aube "$out/bin/aube"
    ln -s aube "$out/bin/aubr"
    ln -s aube "$out/bin/aubx"
    runHook postInstall
  '';

  meta = {
    description = "A fast Node.js package manager";
    homepage = "https://aube.sh";
    mainProgram = "aube";
    sourceProvenance = [ lib.sourceTypes.binaryNativeCode ];
    platforms = [
      "x86_64-linux"
      "aarch64-linux"
      "aarch64-darwin"
    ];
  };
}
