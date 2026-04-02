ARG BUILDER_IMAGE=rust:1.89-bookworm
ARG RUNTIME_IMAGE=debian:bookworm-slim

FROM ${BUILDER_IMAGE} AS builder

WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config libssl-dev ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
RUN mkdir src \
    && printf 'fn main() { println!("build cache warmup"); }\n' > src/main.rs \
    && cargo build --release --locked \
    && rm -rf src

COPY src ./src

ARG APP_FEATURES=""

RUN find src -type f -exec touch {} + \
    && if [ -n "$APP_FEATURES" ]; then \
        cargo build --release --locked --features "$APP_FEATURES"; \
    else \
        cargo build --release --locked; \
    fi

FROM ${RUNTIME_IMAGE} AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl ffmpeg \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/mimic-shrimp-rs /usr/local/bin/mimic-shrimp-rs
COPY forms ./forms

RUN mkdir -p /app/learning_data \
    && chown -R 65534:65534 /app

ENV APP_NAME=mimic-shrimp-rs
ENV SERVER_ADDR=0.0.0.0:7878
ENV RUST_LOG=info
ENV WEIXIN_FFMPEG_BIN=/usr/bin/ffmpeg

EXPOSE 7878

USER 65534:65534

CMD ["mimic-shrimp-rs"]
