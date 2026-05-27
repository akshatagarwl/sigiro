variable "REGISTRY" {
  default = "ghcr.io/sigiroai"
}

variable "TAG" {
  default = "latest"
}

group "default" {
  targets = ["sigiro"]
}

# Standard build: compiles Sigiro in Docker
target "sigiro" {
  context    = "."
  dockerfile = "Dockerfile"
  platforms  = ["linux/amd64", "linux/arm64"]
  tags       = ["${REGISTRY}/sigiro:${TAG}"]
  cache-from = ["type=local,src=.buildx-cache", "type=registry,ref=${REGISTRY}/sigiro:buildcache"]
  cache-to   = ["type=local,dest=.buildx-cache,mode=max", "type=registry,ref=${REGISTRY}/sigiro:buildcache,mode=max"]
}


