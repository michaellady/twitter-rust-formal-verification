# syntax=docker/dockerfile:1.7

# Multi-stage Rust build for the verified twitter server.
# Stage 1 builds a release binary AND writes /etc/version.json (since
# distroless final stage has no /bin/sh to run printf).
# Stage 2 ships only the binary + version.json on a distroless base.

FROM rust:1.95.0-bookworm AS builder
WORKDIR /build
COPY . .

ARG GIT_SHA=dev
ARG IMAGE_DIGEST=sha256:dev

RUN cargo build --release -p server
RUN printf '{"git_sha":"%s","image_digest":"%s"}\n' "$GIT_SHA" "$IMAGE_DIGEST" > /tmp/version.json

FROM gcr.io/distroless/cc-debian12:nonroot
WORKDIR /app
COPY --from=builder /build/target/release/server /app/server
COPY --from=builder /tmp/version.json /etc/version.json

EXPOSE 8080
ENV PORT=8080 RUST_LOG=info
ENTRYPOINT ["/app/server"]
