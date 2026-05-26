variable "REGISTRY" {
  default = "ghcr.io/sigiroai"
}

variable "TAG" {
  default = "latest"
}

group "default" {
  targets = ["sigiro"]
}

# Standard build: compiles spiced inside Docker (slow, ~15min cold)
target "sigiro" {
  context    = "."
  dockerfile = "Dockerfile"
  platforms  = ["linux/amd64", "linux/arm64"]
  tags       = ["${REGISTRY}/sigiro:${TAG}"]
  cache-from = ["type=local,src=.buildx-cache", "type=registry,ref=${REGISTRY}/sigiro:buildcache"]
  cache-to   = ["type=local,dest=.buildx-cache,mode=max", "type=registry,ref=${REGISTRY}/sigiro:buildcache,mode=max"]
}

# Hybrid build: uses pre-built spiced binaries from spiced-binaries/ (fast, ~3min)
# Use: just image-hybrid
target "sigiro-hybrid" {
  context    = "."
  dockerfile = "Dockerfile.hybrid"
  platforms  = ["linux/amd64", "linux/arm64"]
  tags       = ["${REGISTRY}/sigiro:${TAG}-hybrid"]
  cache-from = ["type=local,src=.buildx-cache-hybrid", "type=registry,ref=${REGISTRY}/sigiro:buildcache-hybrid"]
  cache-to   = ["type=local,dest=.buildx-cache-hybrid,mode=max", "type=registry,ref=${REGISTRY}/sigiro:buildcache-hybrid,mode=max"]
  contexts   = {
    "spiced-binaries" = "type=local,path=spiced-binaries"
  }
}

# Alias for convenience
group "hybrid" {
  targets = ["sigiro-hybrid"]
}
