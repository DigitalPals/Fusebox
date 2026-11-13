# Drop-in for CLIProxyAPI's image: same working directory, config path, auth
# directory and ports, so an existing docker-compose.yml keeps working.

# The Rust version is rust-toolchain.toml's.
FROM rust:1.100.0-bookworm AS dependencies
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY .cargo .cargo
# Cache compiled dependencies by toolchain, manifests, and SQLite settings.
# A minimal root builds both runtime and build dependencies without UI/source.
# What that root built of Fusebox itself is removed again, or the next stage would keep
# its empty build script's result. Cargo keeps a package's own files in
# target/release/build/<package> since Rust 1.100; an older Rust needs other paths.
RUN mkdir src \
 && printf 'fn main() {}\n' > src/main.rs \
 && printf 'fn main() {}\n' > build.rs
# Dependency sources are named by crate rather than by their path here (shorter panic
# and log locations, a smaller binary); both stages pass the same flags.
RUN cargo fetch --locked --target "$(rustc -vV | sed -n 's/^host: //p')" \
 && export RUSTFLAGS="$(for src in "$CARGO_HOME"/registry/src/*/; do printf -- '--remap-path-prefix=%s= ' "$src"; done)" \
 && cargo build --release --locked \
 && rm -rf target/release/fusebox target/release/fusebox.d target/release/build/fusebox

FROM dependencies AS build
COPY build.rs ./
COPY src src
COPY ui ui
# Stamp the commit after dependency compilation (the context has no .git).
ARG FUSEBOX_COMMIT=""
ENV FUSEBOX_COMMIT=${FUSEBOX_COMMIT}
# On x86-64, build.rs has Rust's lld pack relative relocations (Debian 12's glibc
# supports RELR) and fold identical functions. Other architectures keep their
# usual linker settings.
RUN export RUSTFLAGS="$(for src in "$CARGO_HOME"/registry/src/*/; do printf -- '--remap-path-prefix=%s= ' "$src"; done)" \
 && cargo build --release --locked \
 && target/release/fusebox --version \
 && if [ "$(uname -m)" = x86_64 ]; then readelf --dynamic target/release/fusebox | grep -q '(RELR)'; fi
# CLIProxyAPI's image runs ./CLIProxyAPI from /CLIProxyAPI and keeps sign-ins in
# /root/.cli-proxy-api; keep both paths working.
RUN mkdir -p /out/CLIProxyAPI /out/root/.cli-proxy-api \
 && cp target/release/fusebox /out/fusebox \
 && ln -s /usr/local/bin/fusebox /out/CLIProxyAPI/CLIProxyAPI

# Runs as an unprivileged user (uid 65532) instead of root: `--target nonroot`, published
# as the `-nonroot` tags. Its config is /home/nonroot/config.yaml and its sign-ins are in
# /home/nonroot/.fusebox; mounted files must be readable and writable by uid 65532.
FROM gcr.io/distroless/cc-debian12:nonroot AS nonroot
LABEL org.opencontainers.image.title="Fusebox" \
      org.opencontainers.image.source="https://github.com/DigitalPals/Fusebox" \
      org.opencontainers.image.description="All your AI subscriptions as one fast API, running as an unprivileged user." \
      org.opencontainers.image.licenses="MIT"
COPY --chmod=644 LICENSE NOTICE /usr/share/doc/fusebox/
COPY --chmod=644 ui/fonts/OFL-*.txt /usr/share/doc/fusebox/licenses/
COPY --from=build /out/fusebox /usr/local/bin/fusebox
ENV HOME=/home/nonroot \
    FUSEBOX_DEFAULT_HOST=0.0.0.0 \
    FUSEBOX_CONFIG=/home/nonroot/config.yaml
WORKDIR /home/nonroot
USER nonroot
EXPOSE 8317 54545 1455 51121
CMD ["/usr/local/bin/fusebox"]

# The default image keeps CLIProxyAPI's layout (root, /CLIProxyAPI), so it drops in.
FROM gcr.io/distroless/cc-debian12 AS default
LABEL org.opencontainers.image.title="Fusebox" \
      org.opencontainers.image.source="https://github.com/DigitalPals/Fusebox" \
      org.opencontainers.image.description="All your AI subscriptions as one fast API. Drop-in for CLIProxyAPI." \
      org.opencontainers.image.licenses="MIT"
COPY --chmod=644 LICENSE NOTICE /usr/share/doc/fusebox/
COPY --chmod=644 ui/fonts/OFL-*.txt /usr/share/doc/fusebox/licenses/
COPY --from=build /out/fusebox /usr/local/bin/fusebox
COPY --from=build /out/CLIProxyAPI /CLIProxyAPI
COPY --from=build /out/root /root
ENV HOME=/root \
    FUSEBOX_DEFAULT_HOST=0.0.0.0
WORKDIR /CLIProxyAPI
# API + dashboard, then the OAuth callback ports (Claude, Codex, Antigravity).
EXPOSE 8317 54545 1455 51121
CMD ["/usr/local/bin/fusebox"]
