FROM node:24.21.0-bookworm-slim

# Install the lockfile's package manager from verified, architecture-specific releases.
ARG TARGETARCH
ARG AUBE_VERSION=2.2.16
RUN set -eu; \
    case "$TARGETARCH" in \
      amd64) platform=x86_64; digest=f459e96c1eba2068ea921875748fe1777e78b919886f2a83058b8c5c50e139ef ;; \
      arm64) platform=aarch64; digest=a3b3a44ebff731672741c41d65b806dc8e138252f81e50240e3f8a359deb065d ;; \
      *) echo "Unsupported architecture: $TARGETARCH" >&2; exit 1 ;; \
    esac; \
    apt-get update; \
    apt-get install -y --no-install-recommends ca-certificates curl; \
    curl -fsSL --retry 3 "https://github.com/aubepkg/aube/releases/download/v${AUBE_VERSION}/aube-v${AUBE_VERSION}-${platform}-unknown-linux-gnu.tar.gz" -o /tmp/aube.tar.gz; \
    echo "${digest}  /tmp/aube.tar.gz" | sha256sum -c -; \
    tar -xzf /tmp/aube.tar.gz -C /usr/local/bin aube; \
    rm -f /tmp/aube.tar.gz; \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY package.json aube-lock.yaml .npmrc ./
RUN aube install --frozen-lockfile
COPY . .
RUN aube run build
# Aube's lockfile links dependencies to /root/.cache/aube/virtual-store.
# Allow traversal to those read-only packages without allowing directory listing.
RUN chmod 0711 /root

# The Eve server and the independently supervised polling worker share this image.
USER node
EXPOSE 3000
CMD ["aube", "run", "start"]
