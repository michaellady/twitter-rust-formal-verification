# syntax=docker/dockerfile:1.7

# Multi-stage Rust build for the verified twitter server.
# Stage 1 builds a static-ish release binary; stage 2 ships only the
# binary on a distroless base. /etc/version.json is injected after the
# digest is known (see verify.yml build-image-main job).

FROM rust:1.95.0-bookworm AS builder
WORKDIR /build
COPY . .
RUN cargo build --release -p server

FROM gcr.io/distroless/cc-debian12:nonroot
WORKDIR /app
COPY --from=builder /build/target/release/server /app/server

# Build args injected by verify.yml second build pass after digest is computed.
ARG GIT_SHA=dev
ARG IMAGE_DIGEST=sha256:dev
RUN ["/bin/sh", "-c", "printf '{\"git_sha\":\"%s\",\"image_digest\":\"%s\"}\\n' \"$GIT_SHA\" \"$IMAGE_DIGEST\" > /etc/version.json"]
# (distroless:cc has /bin/sh via the cc-debian variant; if we shrink to
# distroless:static later, switch to a busybox COPY.)

EXPOSE 8080
ENV PORT=8080 RUST_LOG=info
ENTRYPOINT ["/app/server"]
