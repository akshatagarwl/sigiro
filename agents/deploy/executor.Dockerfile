FROM oven/bun:1.3.14-debian@sha256:9dba1a1b43ce28c9d7931bfc4eb00feb63b0114720a0277a8f939ae4dfc9db6f

ARG CLAUDE_CODE_VERSION=2.1.252
ARG CODEX_VERSION=0.152.0

ENV BUN_INSTALL=/opt/agent-cli
ENV PATH=/opt/agent-cli/bin:/usr/local/bin:/usr/bin:/bin

WORKDIR /app

RUN mkdir -p /opt/agent-cli /home/agent /workspace \
    && chown -R 65532:65532 /opt/agent-cli /home/agent /workspace \
    && BUN_INSTALL=/opt/agent-cli bun add --global \
      "@anthropic-ai/claude-code@${CLAUDE_CODE_VERSION}" \
      "@openai/codex@${CODEX_VERSION}"

COPY --chown=65532:65532 package.json bun.lock ./
RUN bun install --frozen-lockfile --production

COPY --chown=65532:65532 src ./src
COPY --chown=65532:65532 executor ./executor

USER 65532:65532

ENTRYPOINT ["bun", "executor/main.ts"]
