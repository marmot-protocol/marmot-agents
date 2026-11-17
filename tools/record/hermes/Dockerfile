# Hermes Agent with the recording plugin and nothing else: no Marmot, no relays, no keys of ours.
# Build from the repository root:
#   docker build -f tools/record/hermes/Dockerfile -t marmot-agents-record-hermes tools/record/hermes
# Hermes pins its dependencies for Python 3.14 and later.
FROM python:3.14-slim-bookworm

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates git tini \
    && rm -rf /var/lib/apt/lists/*

# Pin the Hermes commit. Change it to record a newer Hermes, then rebuild.
ARG HERMES_AGENT_REF=40d1e95f37e409f9f618f32c170661c66b86a4f4
# Hermes pins every package in uv.lock, with overrides pip cannot resolve, so install with uv.
RUN pip install --no-cache-dir uv \
    && git clone --filter=blob:none --no-checkout https://github.com/NousResearch/hermes-agent.git /opt/hermes-agent \
    && git -C /opt/hermes-agent fetch --depth 1 origin "$HERMES_AGENT_REF" \
    && git -C /opt/hermes-agent checkout FETCH_HEAD \
    && cd /opt/hermes-agent \
    && UV_PROJECT_ENVIRONMENT=/opt/venv uv sync --frozen --no-dev --no-cache

COPY plugin /opt/recorder-plugin
COPY entrypoint.sh /usr/local/bin/record-entrypoint
RUN chmod 0755 /usr/local/bin/record-entrypoint && useradd --create-home --uid 1000 agent \
    && mkdir -p /data /out /work && chown agent:agent /data /out /work

USER agent
ENV HERMES_HOME=/data/hermes-home \
    PATH=/opt/venv/bin:/usr/local/bin:/usr/bin:/bin \
    RECORDER_OUT=/out/recording.jsonl \
    RECORDER_SCENARIO=/scenario.json \
    RECORDER_ALLOW_ALL_USERS=true
WORKDIR /work
VOLUME ["/data"]
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/record-entrypoint"]
