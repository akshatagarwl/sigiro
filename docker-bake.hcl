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

target "sigiro" {
  context    = "."
  dockerfile = "Dockerfile"
  platforms  = ["linux/amd64", "linux/arm64"]
  tags       = ["${REGISTRY}/sigiro:${TAG}"]
  cache-from = PUSH_CACHE == "true" ? ["type=local,src=.buildx-cache", "type=registry,ref=${REGISTRY}/sigiro:buildcache"] : ["type=local,src=.buildx-cache"]
  cache-to   = PUSH_CACHE == "true" ? ["type=local,dest=.buildx-cache,mode=max", "type=registry,ref=${REGISTRY}/sigiro:buildcache,mode=max"] : ["type=local,dest=.buildx-cache,mode=max"]
}
