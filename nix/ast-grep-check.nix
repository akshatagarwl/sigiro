# The structural Rust lint (sgconfig.yml, ast-grep/): first the rules' own
# valid/invalid examples, then a scan of src/ that fails on any error-level
# finding.
{
  lib,
  runCommand,
  ast-grep,
}:
let
  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../sgconfig.yml
      ../ast-grep
      ../src
    ];
  };
in
runCommand "sigiro-ast-grep" { nativeBuildInputs = [ ast-grep ]; } ''
  cd ${src}
  ast-grep test --skip-snapshot-tests
  ast-grep scan --error
  touch $out
''
