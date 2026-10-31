# Development and CI image. The Rust version comes from rust-toolchain.toml,
# so the toolchain is pinned in one place.
FROM debian:trixie-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential ca-certificates curl git pkg-config xz-utils \
    && rm -rf /var/lib/apt/lists/*

# What VS Code needs to start without a screen, for the extension's
# integration tests: a virtual display and the libraries Electron loads.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        xvfb xauth libasound2t64 libgbm1 libgtk-3-0t64 libnss3 libsecret-1-0 \
        libxkbfile1 libxss1 \
    && rm -rf /var/lib/apt/lists/*

# Node.js, for the VS Code extension's grammar tests and packaging. Debian's
# Node is too old for the packaging tool, so an official build is pinned here
# by version and checksum.
RUN case "$(dpkg --print-architecture)" in \
        amd64) arch=x64 sum=472655581fb851559730c48763e0c9d3bc25975c59d518003fc0849d3e4ba0f6 ;; \
        arm64) arch=arm64 sum=f3d5a797b5d210ce8e2cb265544c8e482eaedcb8aa409a8b46da7e8595d0dda0 ;; \
        *) echo "no Node.js build for this architecture" >&2; exit 1 ;; \
    esac \
    && curl -fsSLo /tmp/node.tar.xz "https://nodejs.org/dist/v24.15.0/node-v24.15.0-linux-$arch.tar.xz" \
    && echo "$sum  /tmp/node.tar.xz" | sha256sum -c - \
    && tar -xJf /tmp/node.tar.xz -C /usr/local --strip-components=1 \
        --exclude='*.md' --exclude=LICENSE \
    && rm /tmp/node.tar.xz

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
