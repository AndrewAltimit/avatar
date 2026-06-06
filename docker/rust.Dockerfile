# Reproducible Rust build/test environment for the avatar tools.
#
# Runs as a UID/GID matching the host user so files written into bind-mounted
# volumes (target/, generated assets) come back owned by the host user instead
# of root. Mirrors the pattern used in legend-of-legaia-re's Ghidra image.
#
# Build:  USER_ID=$(id -u) GROUP_ID=$(id -g) docker compose build rust
# Use:    docker compose run --rm rust cargo test --workspace
#
# Note: this image only covers the Rust crates. It deliberately does NOT contain
# Unity or the VRChat SDK — avatar upload remains a manual step in the Unity
# editor (interactive VRChat-account login; see PLAN.md §5).
FROM rust:1.95-slim-bookworm

ARG USER_ID=1000
ARG GROUP_ID=1000

RUN set -eux; \
    if ! getent group "${GROUP_ID}" >/dev/null; then \
        groupadd -g "${GROUP_ID}" builder; \
    fi; \
    if ! getent passwd "${USER_ID}" >/dev/null; then \
        useradd -u "${USER_ID}" -g "${GROUP_ID}" -m -s /bin/bash builder; \
    fi

RUN rustup component add rustfmt clippy

WORKDIR /work
USER ${USER_ID}:${GROUP_ID}
