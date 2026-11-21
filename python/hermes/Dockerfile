# Hermes Agent with the Marmot plugin: what the Hermes scenario runs (crates/marmot-agent/tests/hermes.rs).
# No Marmot keys, relays or accounts: the plugin reaches the core only through its socket, which is
# bind-mounted at run time with the plugin's token. Build from the repository root:
#   docker build -f python/hermes/Dockerfile -t marmot-agents-hermes python
# python/hermes/run.sh builds it when it is missing.
FROM python:3.14.10-slim-bookworm

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates git tini \
    && rm -rf /var/lib/apt/lists/*

# Pin the Hermes commit, and say which release it is: Hermes does not tell a plugin (D-044).
ARG HERMES_AGENT_REF=40d1e95f37e409f9f618f32c170661c66b86a4f4
ARG HERMES_RELEASE=0.23.2
# Hermes pins every package in uv.lock, with overrides pip cannot resolve, so install with uv.
RUN pip install --no-cache-dir uv==0.13.9 \
    && git clone --filter=blob:none --no-checkout https://github.com/NousResearch/hermes-agent.git /opt/hermes-agent \
    && git -C /opt/hermes-agent fetch --depth 1 origin "$HERMES_AGENT_REF" \
    && git -C /opt/hermes-agent checkout FETCH_HEAD \
    && cd /opt/hermes-agent \
    && UV_PROJECT_ENVIRONMENT=/opt/venv uv sync --frozen --no-dev --no-cache

# The user Hermes runs as. Its id must be the one that owns the core's socket folder on the host.
ARG UID=1000
ARG GID=1000
COPY marmot_hermes /opt/marmot-plugin
COPY hermes/entrypoint.sh /usr/local/bin/hermes-entrypoint
RUN chmod 0755 /usr/local/bin/hermes-entrypoint \
    && rm -rf /opt/marmot-plugin/__pycache__ \
    && groupadd --gid "$GID" agent && useradd --create-home --uid "$UID" --gid "$GID" agent \
    && mkdir -p /data /work /run/marmot-agent && chown agent:agent /data /work

USER agent
ENV HERMES_HOME=/data/hermes-home \
    HERMES_VERSION=$HERMES_RELEASE \
    MARMOT_AGENT_HOME=/run/marmot-agent \
    MARMOT_ALLOW_ALL_USERS=true \
    PATH=/opt/venv/bin:/usr/local/bin:/usr/bin:/bin
# Enabling a plugin the first time makes Hermes install a runtime into its home, which takes a while
# and needs the network. Do it here, so a new volume on /data starts from a home that has it.
RUN mkdir -p "$HERMES_HOME/plugins" && cp -r /opt/marmot-plugin "$HERMES_HOME/plugins/marmot" \
    && hermes plugins enable marmot
WORKDIR /work
VOLUME ["/data"]
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/hermes-entrypoint"]
