# Goose, speaking ACP on standard input and output, and nothing else: no Marmot, no relays, no keys.
# Build:
#   docker build -t marmot-agents-goose tools/goose
# `goose-acp.sh` builds it when it is missing and runs it; see the comment there.
# Debian 13.6 (trixie-slim), pinned by digest.
FROM debian:trixie-slim@sha256:d7e12182ce18b85b93007c1dedf31f2d29e01ccf3182cc4017c709b6259bc132

# Python and git are for the demo project the scenarios run in, and for what the agent runs there.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl python3 git libdbus-1-3 libxcb1 \
    && rm -rf /var/lib/apt/lists/*

# Pin the Goose release and its checksum. Change both to use a newer Goose, then rebuild.
ARG GOOSE_VERSION=v1.59.2
ARG GOOSE_SHA256=c63cf82953e968d84b7542eb38a621dfdb5050b967728ed363517be5cdc666ea
RUN curl -fsSL -o /tmp/goose.tar.gz \
        "https://github.com/aaif-goose/goose/releases/download/${GOOSE_VERSION}/goose-x86_64-unknown-linux-gnu.tar.gz" \
    && echo "${GOOSE_SHA256}  /tmp/goose.tar.gz" | sha256sum -c - \
    && mkdir /tmp/goose && tar -xzf /tmp/goose.tar.gz -C /tmp/goose \
    && install -m 0755 "$(find /tmp/goose -type f -name goose | head -n 1)" /usr/local/bin/goose \
    && rm -rf /tmp/goose /tmp/goose.tar.gz \
    && goose --version

COPY entrypoint.sh /usr/local/bin/goose-entrypoint
RUN chmod 0755 /usr/local/bin/goose-entrypoint
ENTRYPOINT ["/usr/local/bin/goose-entrypoint"]
