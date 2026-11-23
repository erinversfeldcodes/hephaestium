# The control plane as a release, built here and run on a slim image.
#
# Build from the repository's root, which holds the dashboard this image serves:
#   docker build -f control-plane/Dockerfile -t hephaestium-control-plane .
#
# All three images are pinned by digest, so the same source builds the same image. The runtime
# image must be the Debian release the builder image is built on: the release carries the
# Erlang runtime from the build stage, linked against that release's libraries.
ARG BUILDER=hexpm/elixir:1.19.5-erlang-28.4.3-debian-trixie-20260824-slim@sha256:0e25a8d44495bf0e62d968745427c146ff500aaaff6e962fafc6b07f9b6cd310
ARG NODE=node:24-trixie-slim@sha256:af02d57b57719b9657a45199a79264a9437940cd55b3c7b3fc99615654f124a0
ARG RUNNER=debian:trixie-20260824-slim@sha256:d7e12182ce18b85b93007c1dedf31f2d29e01ccf3182cc4017c709b6259bc132

# The dashboard is built to static files; nothing of Node goes into the final image.
FROM ${NODE} AS dashboard
WORKDIR /dashboard
COPY dashboard/package.json dashboard/package-lock.json dashboard/.npmrc ./
RUN npm ci
COPY dashboard/index.html dashboard/tsconfig.json dashboard/vite.config.ts ./
COPY dashboard/src src
RUN npm run build

FROM ${BUILDER} AS build

RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential git ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
ENV MIX_ENV=prod

RUN mix local.hex --force && mix local.rebar --force

# Dependencies first, so a change to the code does not fetch and compile them again.
COPY control-plane/mix.exs control-plane/mix.lock ./
RUN mix deps.get --only prod --check-locked
COPY control-plane/config config
RUN mix deps.compile

COPY control-plane/lib lib
COPY control-plane/priv priv
COPY control-plane/rel rel
RUN mix release

FROM ${RUNNER}

RUN apt-get update \
    && apt-get install -y --no-install-recommends libstdc++6 openssl libncurses6 ca-certificates gh \
    && rm -rf /var/lib/apt/lists/*

ENV LANG=C.UTF-8
WORKDIR /app

# `gh` is how the control plane reads and writes GitHub (it takes GH_TOKEN from the environment),
# and how a person's reply is made as them. The package is Debian's, at the release's version.

# Not root: nothing the control plane does needs it.
RUN useradd --system --home /app --shell /usr/sbin/nologin heph
COPY --from=build --chown=heph:heph /app/_build/prod/rel/control_plane ./
COPY --from=dashboard --chown=heph:heph /dashboard/dist /app/dashboard
USER heph

# The API serves the dashboard from here when it is on (HEPH_API_PORT); with the API off,
# nothing reads it.
ENV HEPH_API_STATIC_DIR=/app/dashboard

# The release is configured at start from environment variables (see config/runtime.exs);
# no setting, key or credential is part of the image.
CMD ["/app/bin/control_plane", "start"]
