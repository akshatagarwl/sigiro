FROM node:22.19.0-bookworm-slim@sha256:4a4884e8a44826194dff92ba316264f392056cbe243dcc9fd3551e71cea02b90

ARG TARGETARCH
ARG CELLD_VERSION=v0.4.0
ARG CELLD_AMD64_SHA256=0488628597154725db2f61f85434fb381e1b2535d1e9f097c6d20727cd337973
ARG CELLD_ARM64_SHA256=d95ec7108d2c280144599727ac66106c0123d09c573f5724d3b4d3bc33f7603e
ARG ESBUILD_VERSION=0.25.9

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && case "$TARGETARCH" in \
      amd64) asset=celld-x86_64-unknown-linux-gnu.gz; checksum="$CELLD_AMD64_SHA256" ;; \
      arm64) asset=celld-aarch64-unknown-linux-gnu.gz; checksum="$CELLD_ARM64_SHA256" ;; \
      *) echo "unsupported TARGETARCH: $TARGETARCH" >&2; exit 1 ;; \
    esac \
    && curl -fsSLo /tmp/celld.gz \
      "https://github.com/denoland/celld/releases/download/${CELLD_VERSION}/${asset}" \
    && echo "$checksum  /tmp/celld.gz" | sha256sum -c - \
    && gzip -dc /tmp/celld.gz > /usr/local/bin/celld \
    && chmod 0755 /usr/local/bin/celld \
    && rm /tmp/celld.gz \
    && npm install --global "esbuild@${ESBUILD_VERSION}"

USER 65532:65532
ENTRYPOINT ["/usr/local/bin/celld"]
