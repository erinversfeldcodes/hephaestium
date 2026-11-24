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
ARG RUST=rust:1.99.1-slim-trixie@sha256:f6c4a35d21dbb3c89d7a8d056c193e94fc09037f2b58dd858b09c39a47dd02d7
ARG RUNNER=debian:trixie-20260824-slim@sha256:d7e12182ce18b85b93007c1dedf31f2d29e01ccf3182cc4017c709b6259bc132

# The dashboard is built to static files; nothing of Node goes into the final image.
FROM ${NODE} AS dashboard
WORKDIR /dashboard
COPY dashboard/package.json dashboard/package-lock.json dashboard/.npmrc ./
RUN npm ci
COPY dashboard/index.html dashboard/tsconfig.json dashboard/vite.config.ts ./
COPY dashboard/src src
RUN npm run build

# `heph` is what intake runs to turn an approved ticket into a workload. It is built from the
# Rust sources alone, so a change to the dashboard or the control plane does not rebuild it, and
# on the same Debian release as the runtime image, so it links the libraries that image has. The
# cache mounts keep the dependencies compiled between builds that do change the Rust.
FROM ${RUST} AS heph
# The commit the image is built from. The build context holds no git history, so the commit is
# given, and `heph` is built with it.
ARG HEPHAESTIUM_COMMIT=
ENV HEPHAESTIUM_COMMIT=${HEPHAESTIUM_COMMIT}
WORKDIR /src
COPY rust-toolchain.toml Cargo.toml Cargo.lock ./
COPY crates crates
COPY proto proto
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p heph \
    && cp target/release/heph /heph

# `gh` is GitHub's own release, not Debian's package: the control plane runs `gh api --paginate
# --slurp`, which came in gh 2.48.0 and which the Debian release's gh (2.46) lacks. The release
# tarball is checked against the sha256 in the release's `gh_<version>_checksums.txt`
# (https://github.com/cli/cli/releases), so the build fails if what is downloaded is not what
# was pinned. When raising GH_VERSION, take both hashes from that file, and keep the minimum in
# image-test/check.sh at or below it. Dependabot cannot track a release binary named in an ARG,
# so this is raised by hand. Only the binary leaves this stage, so the image stays small, and
# the stage is rebuilt only when these ARGs or the base image change.
FROM ${RUNNER} AS gh
ARG TARGETARCH
ARG GH_VERSION=2.105.0
ARG GH_SHA256_AMD64=79e7ab5db738c48777b696e374f8b428f0647debfb424f44d2f426370407fe2f
ARG GH_SHA256_ARM64=b1278c75107423d818f1937fd92cafc9e5e9b606631568f68d7fb816f5885a02
RUN apt-get update \
    && apt-get install -y --no-install-recommends curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN set -eu; \
    case "${TARGETARCH:-amd64}" in \
      amd64) arch=amd64; sha=${GH_SHA256_AMD64} ;; \
      arm64) arch=arm64; sha=${GH_SHA256_ARM64} ;; \
      *) echo "no pinned gh for ${TARGETARCH}" >&2; exit 1 ;; \
    esac; \
    name=gh_${GH_VERSION}_linux_${arch}; \
    curl -fsSL -o /gh.tar.gz "https://github.com/cli/cli/releases/download/v${GH_VERSION}/${name}.tar.gz"; \
    echo "${sha}  /gh.tar.gz" | sha256sum -c -; \
    tar -xzf /gh.tar.gz -C / "${name}/bin/gh"; \
    mv "/${name}/bin/gh" /gh

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
    && apt-get install -y --no-install-recommends libstdc++6 openssl libncurses6 ca-certificates \
    && rm -rf /var/lib/apt/lists/*

ENV LANG=C.UTF-8
WORKDIR /app

# `gh` is how the control plane reads and writes GitHub (it takes GH_TOKEN from the environment),
# and how a person's reply is made as them. It is the pinned release from the `gh` stage above.

# Not root: nothing the control plane does needs it.
RUN useradd --system --home /app --shell /usr/sbin/nologin heph
COPY --from=build --chown=heph:heph /app/_build/prod/rel/control_plane ./
COPY --from=heph /heph /usr/local/bin/heph
COPY --from=gh /gh /usr/local/bin/gh
COPY --from=dashboard --chown=heph:heph /dashboard/dist /app/dashboard
USER heph

# The API serves the dashboard from here when it is on (HEPH_API_PORT); with the API off,
# nothing reads it.
ENV HEPH_API_STATIC_DIR=/app/dashboard

# Intake runs this to build a workload (see config/runtime.exs). It reads the ticket through
# `gh` and needs no checkout of the repository and no model key.
ENV HEPH_BIN=/usr/local/bin/heph

# The commit this image was built from, which the control plane keeps on each workload it makes.
# It is given at build time (`--build-arg HEPHAESTIUM_COMMIT=<commit>`); an image built without
# it says its build is unknown.
ARG HEPHAESTIUM_COMMIT=
ENV HEPH_BUILD_COMMIT=${HEPHAESTIUM_COMMIT}

# The release is configured at start from environment variables (see config/runtime.exs);
# no setting, key or credential is part of the image.
CMD ["/app/bin/control_plane", "start"]
