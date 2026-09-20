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

# There is deliberately NO `type=local` cache export.
#
# A docker-container builder already keeps a full content-addressed cache inside
# its own BuildKit state volume, so `cache-to=type=local,dest=.buildx-cache`
# wrote a SECOND copy of those exact layers into the working tree. Worse, the
# local exporter has no garbage collection: BuildKit rewrites index.json each
# build and abandons the previous generation's blobs, so every build since the
# repo's first leaked its whole layer set. Measured: .buildx-cache reached 27 GB
# of which only 2.0 GB was reachable from index.json — 24.2 GB of orphans — while
# the builder held 21.5 GB of the same layers at 100% reclaimable.
#
# The builder's cache is now bounded by ~/.config/buildkit/buildkitd.toml
# (reservedSpace/maxUsedSpace gcpolicy), which self-prunes. That is the single
# source of cache truth for local builds; the registry cache below is the only
# cross-machine sharing path. Recreate the builder with that config via:
#   docker buildx create --name multiplatform --driver docker-container \
#     --config ~/.config/buildkit/buildkitd.toml --bootstrap

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
  cache-from = PUSH_CACHE == "true" ? ["type=registry,ref=${REGISTRY}/sigiro:buildcache"] : []
  cache-to   = PUSH_CACHE == "true" ? ["type=registry,ref=${REGISTRY}/sigiro:buildcache,mode=max"] : []
}

