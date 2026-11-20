FROM rust:trixie

ENV LANG=C.UTF-8
ENV LC_CTYPE=C.UTF-8
ENV PYTHONIOENCODING=utf-8

USER root
SHELL ["/bin/bash", "-c"]

RUN \
  apt-get update && \
  apt-get install -y -q \
    kicad \
    libngspice0 \
    ngspice \
    xvfb \
  && \
# clean up
  apt-get clean && \
  rm -rf /var/lib/apt/lists/*

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

RUN rustup component add rustfmt clippy rust-analyzer
# To deny claude from writing to the cargo config
RUN touch "$CARGO_HOME/config.toml"
