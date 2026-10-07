# Build: compile the app and the CSS/JS bundles.
FROM rust:1.99-slim-bookworm AS builder

RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /usr/src/app

# Build dependencies first so code changes do not rebuild every crate.
COPY Cargo.toml Cargo.lock ./
COPY migration/Cargo.toml migration/Cargo.toml
RUN mkdir -p src/bin migration/src \
 && echo "fn main() {}" > src/bin/main.rs \
 && touch src/lib.rs migration/src/lib.rs \
 && cargo build --release \
 && rm -rf src migration/src

COPY . .
# Make sure cargo sees the real sources as newer than the placeholder build.
RUN touch src/lib.rs src/bin/main.rs migration/src/lib.rs \
 && bin/build-assets \
 && cargo build --release

# Run: a small image with the binary, templates, static files and config.
FROM debian:bookworm-slim

RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --create-home --uid 10001 app

WORKDIR /usr/app
COPY --from=builder /usr/src/app/assets assets
COPY --from=builder /usr/src/app/config config
COPY --from=builder /usr/src/app/target/release/collab-cli collab-cli

USER app
ENV LOCO_ENV=production
# Railway sets PORT; the production config binds 0.0.0.0:$PORT and runs migrations on start.
EXPOSE 8080
ENTRYPOINT ["/usr/app/collab-cli"]
CMD ["start"]
