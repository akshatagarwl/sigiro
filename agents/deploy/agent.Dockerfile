FROM oven/bun:1.3.14-debian@sha256:9dba1a1b43ce28c9d7931bfc4eb00feb63b0114720a0277a8f939ae4dfc9db6f AS bun
FROM node:24.4.1-bookworm-slim@sha256:36ae19f59c91f3303c7a648f07493fe14c4bd91320ac8d898416327bacf1bbfa

ARG TARGETARCH
ARG CELLD_VERSION=v0.4.0
ARG CELLD_AMD64_SHA256=0488628597154725db2f61f85434fb381e1b2535d1e9f097c6d20727cd337973
ARG CELLD_ARM64_SHA256=d95ec7108d2c280144599727ac66106c0123d09c573f5724d3b4d3bc33f7603e

COPY --from=bun /usr/local/bin/bun /usr/local/bin/bun
RUN apt-get update && apt-get install -y --no-install-recommends \
      bash ca-certificates curl git gh openssl supervisor \
    && rm -rf /var/lib/apt/lists/* \
    && case "$TARGETARCH" in \
      amd64) asset=celld-x86_64-unknown-linux-gnu.gz; checksum="$CELLD_AMD64_SHA256" ;; \
      arm64) asset=celld-aarch64-unknown-linux-gnu.gz; checksum="$CELLD_ARM64_SHA256" ;; \
      *) echo "unsupported architecture: $TARGETARCH" >&2; exit 1 ;; \
    esac \
    && curl -fsSLo /tmp/celld.gz "https://github.com/denoland/celld/releases/download/${CELLD_VERSION}/${asset}" \
    && echo "$checksum  /tmp/celld.gz" | sha256sum -c - \
    && gzip -dc /tmp/celld.gz > /usr/local/bin/celld \
    && chmod 0755 /usr/local/bin/celld && rm /tmp/celld.gz

WORKDIR /app
ENV PATH=/app/node_modules/.bin:/usr/local/bin:/usr/bin:/bin \
    CLAUDE_CONFIG_DIR=/var/lib/sigiro-agent/auth/claude \
    GH_CONFIG_DIR=/var/lib/sigiro-agent/auth/github \
    CODEX_HOME=/var/lib/sigiro-agent/auth/codex \
    SIGIRO_EVIDENCE_DIR=/var/lib/sigiro-agent/evidence \
    CELLD_RECEIPTS_DIR=/var/lib/sigiro-agent/receipts \
    EXECUTOR_STATE_DIR=/var/lib/sigiro-agent/jobs \
    SIGIRO_AGENT_PROVIDER=claude
COPY package.json bun.lock ./
RUN bun install --frozen-lockfile \
    && cp -R node_modules/@ewhauser/world-celld/celld-worker /app/world-celld-worker
COPY src ./src
COPY agent ./agent
COPY tsconfig.json ./tsconfig.json
COPY deploy/start-agent.sh deploy/supervisord.conf ./deploy/
RUN bun run build && bun run eve:build \
    && node --input-type=module -e 'import { createRequire } from "node:module"; import { symlinkSync } from "node:fs"; const require=createRequire(import.meta.url); symlinkSync(require.resolve(`@anthropic-ai/claude-agent-sdk-${process.platform}-${process.arch}/claude`), "/usr/local/bin/claude");' \
    && mkdir -p /var/lib/sigiro-agent/eve/.workflow-data \
    && ln -s /var/lib/sigiro-agent/eve/.workflow-data /app/.eve/.workflow-data \
    && chmod +x /app/deploy/start-agent.sh

ENTRYPOINT ["/app/deploy/start-agent.sh"]
