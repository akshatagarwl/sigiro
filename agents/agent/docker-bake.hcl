group "default" {
  targets = ["optimizer"]
}

target "optimizer" {
  context    = "."
  dockerfile = "Dockerfile"
  tags       = ["sigiro-optimizer-agent:local"]
  platforms  = ["linux/amd64"]
}
