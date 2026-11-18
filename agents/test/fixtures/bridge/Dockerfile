# Build once with network access; Eve uses this image with networking disabled.
FROM node:24.4.1-bookworm-slim@sha256:36ae19f59c91f3303c7a648f07493fe14c4bd91320ac8d898416327bacf1bbfa
RUN apt-get update && apt-get install -y --no-install-recommends bash git ca-certificates tar \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /opt/fixture-dependencies
COPY package.json package-lock.json ./
RUN npm ci --ignore-scripts --cache /opt/npm-cache \
    && rm -rf node_modules /root/.npm \
    && chmod -R a+rwX /opt/npm-cache
ENV npm_config_cache=/opt/npm-cache \
    npm_config_offline=true \
    npm_config_audit=false \
    npm_config_fund=false
WORKDIR /workspace
