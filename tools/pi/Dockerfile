# Pi, the coding agent, and nothing else: no Marmot, no relays, no keys of ours. The model key is
# mounted read-only when a container starts, never built in. Build:
#   docker build -t marmot-agents-pi tools/pi
# `pi.sh` builds it on first use and runs Pi in it.
FROM node:24.22.1-bookworm-slim

# Pi's tools run here: a shell, git, and Python for the demo project the recordings use.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates git python3 python3-pytest tini \
    && rm -rf /var/lib/apt/lists/*

# Pin the Pi release. Change it to run or record a newer Pi, then rebuild.
ARG PI_VERSION=1.6.0
RUN npm install --global --no-fund --no-audit "@earendil-works/pi-coding-agent@${PI_VERSION}" \
    && npm cache clean --force

COPY entrypoint.sh /usr/local/bin/pi-entrypoint
RUN chmod 0755 /usr/local/bin/pi-entrypoint && mkdir -p /state && chmod 0777 /state

# Everything Pi keeps (its settings and its sessions) lives under /state, which pi.sh mounts.
ENV PI_CODING_AGENT_DIR=/state/agent \
    HOME=/state/home \
    PI_SKIP_VERSION_CHECK=1 \
    PI_TELEMETRY=0
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/pi-entrypoint"]
CMD ["pi", "--version"]
