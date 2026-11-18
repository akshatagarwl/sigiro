FROM node:24.20.0-bookworm-slim AS aube
ARG TARGETARCH
RUN apt-get update \
    && apt-get install --no-install-recommends -y ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && case "$TARGETARCH" in \
         amd64) url='https://github.com/aubepkg/aube/releases/download/v2.2.16/aube-v2.2.16-x86_64-unknown-linux-gnu.tar.gz'; sha='f459e96c1eba2068ea921875748fe1777e78b919886f2a83058b8c5c50e139ef' ;; \
         arm64) url='https://github.com/aubepkg/aube/releases/download/v2.2.16/aube-v2.2.16-aarch64-unknown-linux-gnu.tar.gz'; sha='a3b3a44ebff731672741c41d65b806dc8e138252f81e50240e3f8a359deb065d' ;; \
         *) echo "unsupported TARGETARCH: $TARGETARCH" >&2; exit 1 ;; \
       esac \
    && curl --fail --silent --show-error --location "$url" -o /tmp/aube.tar.gz \
    && echo "$sha  /tmp/aube.tar.gz" | sha256sum -c - \
    && tar -xzf /tmp/aube.tar.gz -C /usr/local/bin aube \
    && rm /tmp/aube.tar.gz

FROM aube AS dependencies
WORKDIR /app
COPY package.json aube-lock.yaml .npmrc ./
COPY patches ./patches
RUN aube install --frozen-lockfile

FROM dependencies AS build
COPY tsconfig.json tsconfig.build.json ./
COPY src ./src
RUN aube run build

FROM aube AS production-dependencies
WORKDIR /app
COPY package.json aube-lock.yaml .npmrc ./
COPY patches ./patches
RUN aube install --frozen-lockfile --production --ignore-scripts

FROM node:24.20.0-bookworm-slim
ENV NODE_ENV=production
WORKDIR /app
COPY --from=production-dependencies --chown=node:node /app/node_modules ./node_modules
COPY --from=build --chown=node:node /app/dist ./dist
COPY --chown=node:node package.json ./
USER node
EXPOSE 3100
HEALTHCHECK --interval=10s --timeout=3s --retries=3 CMD ["node", "-e", "fetch('http://127.0.0.1:3100/health').then(r=>{if(!r.ok)process.exit(1)}).catch(()=>process.exit(1))"]
CMD ["node", "dist/server.js"]
