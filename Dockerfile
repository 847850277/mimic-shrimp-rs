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
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/rs-tool-call /usr/local/bin/rs-tool-call

ENV APP_NAME=rs-tool-call
ENV SERVER_ADDR=0.0.0.0:7878
ENV RUST_LOG=info

EXPOSE 7878

USER 65534:65534

CMD ["rs-tool-call"]
