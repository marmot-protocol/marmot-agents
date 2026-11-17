# OpenClaw with the recording plugin and nothing else: no Marmot, no relays, no keys of ours.
# Build from the repository root:
#   docker build -f tools/record/openclaw/Dockerfile -t marmot-agents-record-openclaw tools/record/openclaw
FROM node:24.22.1-bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates git tini \
    && rm -rf /var/lib/apt/lists/*

# Pin the OpenClaw release. Change it to record a newer OpenClaw, then rebuild.
ARG OPENCLAW_VERSION=2026.11.5
RUN npm install --global --no-fund --no-audit "openclaw@${OPENCLAW_VERSION}" \
    && npm cache clean --force

COPY plugin /opt/recorder-plugin
COPY entrypoint.sh /usr/local/bin/record-entrypoint
RUN chmod 0755 /usr/local/bin/record-entrypoint \
    && mkdir -p /data /out /work && chown node:node /data /out /work

USER node
ENV OPENCLAW_HOME=/data/openclaw-home \
    HOME=/data/openclaw-home \
    RECORDER_OUT=/out/recording.jsonl \
    RECORDER_SCENARIO=/scenario.json
WORKDIR /work
VOLUME ["/data"]
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/record-entrypoint"]
