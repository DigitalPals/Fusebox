# Drop-in for CLIProxyAPI's image: same working directory, config path, auth
# directory and ports, so an existing docker-compose.yml keeps working.

FROM rust:1-bookworm AS dependencies
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY .cargo .cargo
# Cache compiled dependencies by toolchain, manifests, and SQLite settings.
# A minimal root builds both runtime and build dependencies without UI/source.
RUN mkdir src \
 && printf 'fn main() {}\n' > src/main.rs \
 && printf 'fn main() {}\n' > build.rs
RUN case "$(uname -m)" in \
      x86_64) cargo rustc --release --locked --bin fusebox -- \
        -C link-arg=-Wl,-z,pack-relative-relocs -C link-arg=-Wl,--icf=all ;; \
      *) cargo build --release --locked ;; \
    esac \
 && rm -rf target/release/fusebox target/release/fusebox.d \
      target/release/deps/fusebox-* target/release/build/fusebox-* \
      target/release/.fingerprint/fusebox-*

FROM dependencies AS build
COPY build.rs ./
COPY src src
COPY ui ui
# Stamp the commit after dependency compilation (the context has no .git).
ARG FUSEBOX_COMMIT=""
ENV FUSEBOX_COMMIT=${FUSEBOX_COMMIT}
# On x86-64, Rust links with lld: pack relative relocations (Debian 12's glibc
# supports RELR) and fold identical functions. Other architectures keep their
# usual linker settings.
RUN case "$(uname -m)" in \
      x86_64) cargo rustc --release --locked --bin fusebox -- \
        -C link-arg=-Wl,-z,pack-relative-relocs -C link-arg=-Wl,--icf=all ;; \
      *) cargo build --release --locked ;; \
    esac \
 && target/release/fusebox --version
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
