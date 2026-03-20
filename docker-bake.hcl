variable "REGISTRY" {
  default = "ghcr.io/sigiroai"
}

variable "TAG" {
  default = "latest"
}

group "default" {
  targets = ["sigiro"]
}

target "sigiro" {
  context    = "."
  dockerfile = "Dockerfile"
  platforms  = ["linux/amd64", "linux/arm64"]
  tags       = ["${REGISTRY}/sigiro:${TAG}"]
  cache-from = ["type=registry,ref=${REGISTRY}/sigiro:buildcache"]
  cache-to   = ["type=registry,ref=${REGISTRY}/sigiro:buildcache,mode=max"]
}

# Single-platform build for local dev (fast, no QEMU)
target "local" {
  inherits  = ["sigiro"]
  platforms = []
  tags      = ["sigiro:local"]
  cache-to  = []
  output    = ["type=docker"]
}
