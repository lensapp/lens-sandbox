# syntax=docker/dockerfile:1
#
# Reproduces the `nft-build` stage of the upstream `lens-sandbox-core`
# Dockerfile so we can bake the same statically-linked `nft` binary into the
# lns-service guest tree. Upstream only ships this binary inside its loader
# image; lns-service runs an unprovisioned microVM, so we build the binary
# ourselves and embed it via `include_bytes!`.
#
# Invoke via `scripts/build-static-nft.sh`. NFTABLES_VERSION + sha256 must
# match the upstream Dockerfile pin — bumps go in lockstep with upstream.

FROM alpine:3.23 AS build

ARG NFTABLES_VERSION
ARG NFTABLES_SHA256

# Same package set as the upstream Dockerfile's nft-build stage. The
# `*-static` apks supply `.a` archives so `-all-static` succeeds.
RUN apk add --no-cache \
      build-base bison flex \
      nftables-dev nftables-static \
      libnftnl-dev libmnl-dev libmnl-static \
      jansson-dev jansson-static \
      gmp-dev gmp-static \
      readline-static ncurses-static

# Build static nft from upstream source. `--with-cli=no` drops readline
# (not needed for `nft -f -`); `--with-json` keeps JSON input/output
# (the supervisor only uses the text form today but the cost is small).
RUN cd /tmp && \
    wget -q "https://www.nftables.org/projects/nftables/files/nftables-${NFTABLES_VERSION}.tar.xz" && \
    echo "${NFTABLES_SHA256}  nftables-${NFTABLES_VERSION}.tar.xz" | sha256sum -c - && \
    tar xf "nftables-${NFTABLES_VERSION}.tar.xz" && \
    cd "nftables-${NFTABLES_VERSION}" && \
    LDFLAGS="-static" LIBS="-lnftables -lnftnl -lmnl -ljansson -lgmp" \
      ./configure \
        --disable-shared --enable-static \
        --with-cli=no --with-json && \
    make -j"$(nproc)" LDFLAGS="-all-static" && \
    strip src/nft && \
    cp src/nft /nft

# Export-only stage: `docker buildx build --output type=local` writes the
# final stage's root filesystem to a host directory. `FROM scratch` keeps
# the output to exactly the one file we want.
FROM scratch
COPY --from=build /nft /nft
