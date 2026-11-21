# OpenClaw at a pinned release, with the marmot plugin installed from this package. No keys, no
# Marmot account, no core: the core runs outside, and its socket folder and token are mounted.
# Build from packages/openclaw:
#   docker build -f container/Dockerfile -t marmot-agent-openclaw .
FROM node:24.22.1-bookworm-slim AS build
WORKDIR /plugin
COPY package.json package-lock.json tsconfig.json tsconfig.build.json openclaw.plugin.json ./
RUN npm ci --no-fund --no-audit
COPY src src
RUN npm run build && npm pack --silent && mv marmot-agent-openclaw-*.tgz /marmot-plugin.tgz

FROM node:24.22.1-bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates git tini \
    && rm -rf /var/lib/apt/lists/*

# Pin the OpenClaw release. Change it to run a newer OpenClaw, then rebuild.
ARG OPENCLAW_VERSION=2026.11.5
RUN npm install --global --no-fund --no-audit "openclaw@${OPENCLAW_VERSION}" \
    && npm cache clean --force

COPY --from=build /marmot-plugin.tgz /opt/marmot-plugin.tgz
COPY container/entrypoint.sh /usr/local/bin/marmot-openclaw
RUN chmod 0755 /usr/local/bin/marmot-openclaw && chmod 0644 /opt/marmot-plugin.tgz \
    && mkdir -p /state /marmot /work && chmod 0777 /state /work

# OpenClaw's state, the agent's home (the core's socket folder and the token) and the agent's work folder
# are mounted at these three places.
ENV HOME=/state \
    OPENCLAW_HOME=/state \
    MARMOT_AGENT_HOME=/marmot
WORKDIR /work
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/marmot-openclaw"]
