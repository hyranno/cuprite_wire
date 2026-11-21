# KiCad 10 (kicad-cli, pcbnew and its Python module, the symbol and footprint libraries) on Debian trixie.
# CI also tests with KiCad 9 (Debian's packages on rust:trixie).
FROM kicad/kicad:10.0

ENV LANG=C.UTF-8
ENV LC_CTYPE=C.UTF-8
ENV PYTHONIOENCODING=utf-8

# The image's default user is `kicad`; the devcontainer runs as root.
USER root
SHELL ["/bin/bash", "-c"]

# pkg-config and libssl-dev build cargo-public-api below, which links OpenSSL through curl
# (crates-index); rust:trixie had them.
RUN \
  apt-get update && \
  apt-get install -y -q --no-install-recommends \
    build-essential \
    ca-certificates \
    curl \
    libngspice0 \
    libssl-dev \
    ngspice \
    pkg-config \
    verilator \
    xvfb \
  && \
# clean up
  apt-get clean && \
  rm -rf /var/lib/apt/lists/*

# Rust, installed where the rust images put it.
ENV RUSTUP_HOME=/usr/local/rustup
ENV CARGO_HOME=/usr/local/cargo
ENV PATH=/usr/local/cargo/bin:$PATH
RUN \
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
    sh -s -- -y --no-modify-path --profile minimal --default-toolchain stable \
      --component rustfmt,clippy,rust-analyzer && \
  chmod -R a+w "$RUSTUP_HOME" "$CARGO_HOME"

# Let the ngspice CLI work where /tmp is read-only (see tmpfile_shim.c).
# The wrapper preloads the shim for ngspice only, not for every process.
COPY tmpfile_shim.c /tmp/tmpfile_shim.c
RUN \
  gcc -shared -fPIC -O2 -o /usr/local/lib/libtmpfile_shim.so /tmp/tmpfile_shim.c && \
  rm /tmp/tmpfile_shim.c && \
  printf '%s\n' \
    '#!/bin/sh' \
    'LD_PRELOAD="/usr/local/lib/libtmpfile_shim.so${LD_PRELOAD:+:$LD_PRELOAD}" exec /usr/bin/ngspice "$@"' \
    > /usr/local/bin/ngspice && \
  chmod +x /usr/local/bin/ngspice

# Refactoring aids (not run by CI): duplicate detection, unused dependencies and the public API.
# cargo-public-api builds rustdoc JSON, which needs a nightly toolchain.
# cargo-sweep prunes stale artifacts from target/ (see postStartCommand in devcontainer.json).
RUN \
  cargo install --locked \
    similarity-rs@0.5.0 \
    cargo-sweep@0.8.0 \
    cargo-shear@1.14.1 \
    cargo-public-api@0.52.0 \
  && \
  rustup toolchain install nightly --profile minimal && \
  rm -rf "$CARGO_HOME/registry"
# To deny claude from writing to the cargo config
RUN touch "$CARGO_HOME/config.toml"
