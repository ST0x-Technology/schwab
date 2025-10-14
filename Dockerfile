FROM ubuntu:latest AS builder

RUN apt update -y
RUN apt install curl git -y
RUN curl --proto '=https' --tlsv1.2 -sSf -L https://install.determinate.systems/nix | sh -s -- install linux \
  --extra-conf "sandbox = false" \
  --extra-conf "experimental-features = nix-command flakes" \
  --init none \
  --no-confirm
ENV PATH="${PATH}:/nix/var/nix/profiles/default/bin"

WORKDIR /app

COPY flake.nix flake.lock ./
RUN nix develop --command echo "Nix dev env ready"

COPY Cargo.toml Cargo.lock ./

RUN mkdir -p src/bin && \
    echo 'fn main() {}' > src/lib.rs && \
    echo 'fn main() {}' > src/bin/server.rs

RUN nix develop --command cargo chef prepare --recipe-path recipe.json

RUN rm -rf src

RUN nix develop --command cargo chef cook --release --recipe-path recipe.json

COPY . .

RUN nix run .#prepSolArtifacts

RUN nix develop --command bash -c ' \
    export DATABASE_URL=sqlite:///tmp/build_db.sqlite && \
    sqlx database create && \
    sqlx migrate run \
'

RUN nix develop --command bash -c ' \
    export DATABASE_URL=sqlite:///tmp/build_db.sqlite && \
    cargo test -q \
'

RUN nix develop --command bash -c ' \
    export DATABASE_URL=sqlite:///tmp/build_db.sqlite && \
    cargo build --release --bin server \
'

# Fix binary interpreter path to use standard Linux paths
RUN apt-get update && apt-get install -y patchelf && \
    patchelf --set-interpreter /lib64/ld-linux-x86-64.so.2 /app/target/release/server && \
    apt-get remove -y patchelf && apt-get autoremove -y && rm -rf /var/lib/apt/lists/*

FROM debian:12-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        ca-certificates && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy only the compiled binaries from builder stage (now with fixed interpreter paths)
COPY --from=builder /app/target/release/server ./
COPY --from=builder /app/migrations ./migrations

RUN chown -R schwab:schwab /app

ENTRYPOINT ["./server"]
