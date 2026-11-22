# The control plane as a release, built here and run on a slim image.
#
# Build from this directory: docker build -t hephaestium-control-plane .
#
# Both images are pinned by digest, so the same source builds the same image. The runtime
# image must be the Debian release the builder image is built on: the release carries the
# Erlang runtime from the build stage, linked against that release's libraries.
ARG BUILDER=hexpm/elixir:1.19.5-erlang-28.4.3-debian-trixie-20260824-slim@sha256:0e25a8d44495bf0e62d968745427c146ff500aaaff6e962fafc6b07f9b6cd310
ARG RUNNER=debian:trixie-20260824-slim@sha256:d7e12182ce18b85b93007c1dedf31f2d29e01ccf3182cc4017c709b6259bc132

FROM ${BUILDER} AS build

RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential git ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
ENV MIX_ENV=prod

RUN mix local.hex --force && mix local.rebar --force

# Dependencies first, so a change to the code does not fetch and compile them again.
COPY mix.exs mix.lock ./
RUN mix deps.get --only prod --check-locked
COPY config config
RUN mix deps.compile

COPY lib lib
COPY priv priv
COPY rel rel
RUN mix release

FROM ${RUNNER}

RUN apt-get update \
    && apt-get install -y --no-install-recommends libstdc++6 openssl libncurses6 ca-certificates \
    && rm -rf /var/lib/apt/lists/*

ENV LANG=C.UTF-8
WORKDIR /app

# Not root: nothing the control plane does needs it.
RUN useradd --system --home /app --shell /usr/sbin/nologin heph
COPY --from=build --chown=heph:heph /app/_build/prod/rel/control_plane ./
USER heph

# The release is configured at start from environment variables (see config/runtime.exs);
# no setting, key or credential is part of the image.
CMD ["/app/bin/control_plane", "start"]
