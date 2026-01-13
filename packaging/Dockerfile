FROM rust:1.83 AS server-builder
RUN apt-get update && apt-get install -y --no-install-recommends musl-tools pkg-config ca-certificates && rm -rf /var/lib/apt/lists/*
ARG SERVER_DIR=.
ARG SERVER_TARGET=x86_64-unknown-linux-musl
RUN rustup target add ${SERVER_TARGET}
WORKDIR /src/nasserver
COPY Cargo.toml Cargo.lock* ./
RUN mkdir -p src
COPY src ./src
RUN cargo build --release --target ${SERVER_TARGET}
RUN mkdir -p /artifacts/server && cp target/${SERVER_TARGET}/release/pnas-server /artifacts/server/pnas-server

FROM alpine:3.19 AS packager
RUN apk add --no-cache squashfs-tools
ARG VERSION=0.1.0
ARG SYSEXT_ID=pnas-server
ARG OS_ID=fedora
ARG OS_VERSION_ID=40
ARG COMPRESSION=xz
ARG BLOCK_SIZE=1048576
RUN mkdir -p /build_root/usr/bin /build_root/usr/lib/systemd/system /build_root/usr/lib/extension-release.d /out
COPY --from=server-builder /artifacts/server/pnas-server /build_root/usr/bin/pnas-server
COPY packaging/pnas-server.service /build_root/usr/lib/systemd/system/pnas-server.service
RUN printf "ID=%s\nVERSION_ID=%s\nSYSEXT_ID=%s\n" "$OS_ID" "$OS_VERSION_ID" "$SYSEXT_ID" > /build_root/usr/lib/extension-release.d/extension-release.${SYSEXT_ID}
RUN sh -c 'comp="${COMPRESSION}"; bs="${BLOCK_SIZE}"; extra=""; [ "$comp" = "xz" ] && extra="-Xdict-size $bs"; mksquashfs /build_root "/out/pnas-server-v${VERSION}.raw" -noappend -comp "$comp" -b "$bs" $extra'
CMD ["sh","-c","ls -l /out"]
