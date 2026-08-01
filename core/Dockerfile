# handoffd, the reference server.
#
# Two stages: a build stage that compiles the release binary, and a runtime stage that carries the
# binary and nothing else. The runtime image has no cargo, no source, no shell tooling beyond what
# Debian slim ships, and no bootstrap file baked in -- credentials arrive as a mounted secret or an
# environment variable at run time, never in a layer, because an image layer is forever and a
# published image is a distribution channel.
#
# `handoffd` applies its own migrations on start (SECURITY.md, "Running a deployment safely"), so
# the database role it connects as must own its schema. It does NOT need to be a superuser and MUST
# NOT have BYPASSRLS: the policies are installed with FORCE ROW LEVEL SECURITY, so ownership does
# not exempt it from them.

# 1.94, not the workspace's `rust-version = "1.82"`. That field is the MINIMUM this code supports,
# not the toolchain it builds with: a transitive dependency now requires `edition2024`, which Cargo
# stabilised in 1.85, so 1.82 cannot parse its manifest. CI builds with `stable` and so does this,
# pinned to a version rather than floating, so an image rebuild months from now produces the same
# binary rather than whatever stable means that morning.
FROM rust:1.94-slim-bookworm AS build
WORKDIR /src

# Only what cargo needs to link. `pkg-config` and `libssl-dev` are here because the store and the
# delivery adapters pull in TLS; keeping the list explicit means a new dependency that needs a
# system library fails the build loudly rather than at run time.
RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config libssl-dev ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY . .

# `--locked` so the image is built from the committed Cargo.lock and cannot drift from what CI
# tested. A build that needs to update the lockfile is a change to the repository, not to the image.
# `--features sentry` is OUR deployment's choice, not the crate's default. The feature is off by
# default so a fork's binary cannot report anywhere, which is what keeps README's "no phone-home"
# literally true; enabling it here is how this deployment opts in. Even then, no SENTRY_DSN means
# no client and no socket.
RUN cargo build --release --locked -p handoff-server --bin handoffd --features sentry \
    && strip target/release/handoffd

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --shell /usr/sbin/nologin handoff

COPY --from=build /src/target/release/handoffd /usr/local/bin/handoffd
COPY docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh

# Unprivileged. Nothing here needs root, and the process that holds every tenant's receipts is the
# last one that should have it.
USER handoff
WORKDIR /home/handoff

# Bind on all interfaces inside the container; the platform decides what reaches it. Every other
# setting is supplied at run time so this image is not specific to any one deployment.
# `[::]`, not `0.0.0.0`: on a platform whose private network is IPv6, the IPv4 wildcard is
# reachable from the edge proxy and from nothing else. A `::` bind accepts IPv4 too.
ENV HANDOFF_BIND=[::]:8080
EXPOSE 8080

ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
CMD ["serve"]
