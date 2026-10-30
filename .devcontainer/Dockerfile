# Development and CI image. The Rust version comes from rust-toolchain.toml,
# so the toolchain is pinned in one place.
FROM debian:trixie-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential ca-certificates curl git pkg-config \
    && rm -rf /var/lib/apt/lists/*

ARG USERNAME=dev
ARG USER_UID=1000
RUN useradd --create-home --uid "$USER_UID" --shell /bin/bash "$USERNAME"

# Rust lives in the user's home so dev container UID remapping keeps it writable.
ENV RUSTUP_HOME=/home/$USERNAME/.rustup \
    CARGO_HOME=/home/$USERNAME/.cargo \
    CARGO_TARGET_DIR=/home/$USERNAME/target \
    PATH=/home/$USERNAME/.cargo/bin:$PATH

USER $USERNAME
WORKDIR /home/$USERNAME

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --no-modify-path --profile minimal --default-toolchain none

COPY --chown=$USERNAME rust-toolchain.toml /tmp/toolchain/rust-toolchain.toml
RUN cd /tmp/toolchain && rustup toolchain install && rm -rf /tmp/toolchain

# Mount points for the cache volumes, created here so they are owned by the user.
RUN mkdir -p "$CARGO_HOME/registry" "$CARGO_TARGET_DIR"
