variable "REGISTRY" {
  default = "ghcr.io/sigiroai"
}

variable "TAG" {
  default = "latest"
}

# Registry build cache is OFF by default: writing ghcr.io/.../sigiro:buildcache
# needs a packages:write token, and without it the cache-to export fails and
# breaks the whole build (even a plain `just image`). Default keeps caching
# purely local so builds work with zero registry auth. Publishers with a scoped
# token opt in: `PUSH_CACHE=true just image --push` to share cache with CI.
variable "PUSH_CACHE" {
  default = "false"
}

group "default" {
  targets = ["sigiro"]
}

# The downloadable Linux binaries, written to ./dist-bin/linux_{amd64,arm64}/sigiro.
#
#   docker buildx bake dist
#
# Not in the default group: it writes to the working tree, which `just image`
# must not do. It shares every cache layer with `sigiro`, so running both costs
# barely more than running one.
#
# amd64 is not emulated. The builder stage cross-compiles from $BUILDPLATFORM —
# rustc runs on the host arch and the matching cross-gcc links — so an arm64 Mac
# produces the amd64 binary at native speed.
target "dist" {
  inherits  = ["sigiro"]
  target    = "dist"
  tags      = []
  output    = ["type=local,dest=./dist-bin"]
}

target "sigiro" {
  context    = "."
  dockerfile = "Dockerfile"
  platforms  = ["linux/amd64", "linux/arm64"]
  tags       = ["${REGISTRY}/sigiro:${TAG}"]
  cache-from = PUSH_CACHE == "true" ? ["type=local,src=.buildx-cache", "type=registry,ref=${REGISTRY}/sigiro:buildcache"] : ["type=local,src=.buildx-cache"]
  cache-to   = PUSH_CACHE == "true" ? ["type=local,dest=.buildx-cache,mode=max", "type=registry,ref=${REGISTRY}/sigiro:buildcache,mode=max"] : ["type=local,dest=.buildx-cache,mode=max"]
}
