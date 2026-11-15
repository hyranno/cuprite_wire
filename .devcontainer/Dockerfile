FROM rust

ENV LANG=C.UTF-8
ENV LC_CTYPE=C.UTF-8
ENV PYTHONIOENCODING=utf-8

USER root
SHELL ["/bin/bash", "-c"]

RUN \
  apt-get update && \
  apt-get install -y -q \
    kicad \
    ngspice \
  && \
# clean up
  apt-get clean && \
  rm -rf /var/lib/apt/lists/*

RUN rustup component add rustfmt clippy rust-analyzer
