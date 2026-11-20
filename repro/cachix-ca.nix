builtins.derivation {
  name = "sigiro-ca-realisations-probe";
  system = "x86_64-linux";
  builder = "/bin/sh";
  args = [ "-c" "printf 'CA realisation cache probe\\n' > \"$out\"" ];
  __contentAddressed = true;
  outputHashMode = "recursive";
  outputHashAlgo = "sha256";
}
