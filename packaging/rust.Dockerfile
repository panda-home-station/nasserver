# podman build -f rust.Dockerfile -t harbor.pandamicro.com/phs/ci-rust-nasserver:1.87 .

FROM harbor.pandamicro.com/library/rust:1.87
ARG ORAS_VERSION="1.3.0"
RUN apt-get update && apt-get install -y --no-install-recommends musl-tools pkg-config ca-certificates curl git gcc make squashfs-tools && rm -rf /var/lib/apt/lists/*
RUN sh -c 'command -v rustup >/dev/null 2>&1 || curl https://sh.rustup.rs -sSf | sh -s -- -y'
ENV PATH="/root/.cargo/bin:${PATH}"
RUN rustup target add x86_64-unknown-linux-musl
RUN curl -L "https://github.com/oras-project/oras/releases/download/v${ORAS_VERSION}/oras_${ORAS_VERSION}_linux_amd64.tar.gz" -o /tmp/oras.tar.gz && tar -zxf /tmp/oras.tar.gz -C /tmp && install -m 0755 /tmp/oras /usr/local/bin/oras && rm -f /tmp/oras.tar.gz
