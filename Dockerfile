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

# Copy flake files first for better layer caching
COPY flake.nix flake.lock ./
RUN nix develop --command echo "Nix dev env ready"

# Copy only Cargo files for dependency resolution
COPY Cargo.toml Cargo.lock ./

# Create minimal dummy source structure for cargo chef
RUN mkdir -p src/bin && \
    echo 'fn main() {}' > src/lib.rs && \
    echo 'fn main() {}' > src/bin/main.rs && \
    echo 'fn main() {}' > src/bin/auth.rs

# Prepare cargo chef recipe (cached layer if Cargo.toml doesn't change)
RUN nix develop --command cargo chef prepare --recipe-path recipe.json

# Remove dummy source files
RUN rm -rf src

# Cook dependencies using cargo chef (this is the expensive cached layer)
RUN nix develop --command cargo chef cook --release --recipe-path recipe.json

COPY . .

# Build Solidity artifacts
RUN nix run .#prepSolArtifacts

# Set up database and run migrations for SQLx compile-time verification
RUN nix develop --command bash -c ' \
    export DATABASE_URL=sqlite:///tmp/build_db.sqlite && \
    sqlx database create && \
    sqlx migrate run \
'

# Build final Rust binaries (fast since deps are already compiled)
RUN nix develop --command bash -c ' \
    export DATABASE_URL=sqlite:///tmp/build_db.sqlite && \
    cargo build --release --bin main --bin auth \
'

# Fix binary interpreter path to use standard Linux paths
RUN apt-get update && apt-get install -y patchelf && \
    patchelf --set-interpreter /lib64/ld-linux-x86-64.so.2 /app/target/release/main && \
    patchelf --set-interpreter /lib64/ld-linux-x86-64.so.2 /app/target/release/auth && \
    apt-get remove -y patchelf && apt-get autoremove -y && rm -rf /var/lib/apt/lists/*

FROM debian:12-slim

# Install runtime dependencies
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        ca-certificates && \
    rm -rf /var/lib/apt/lists/*

# Create schwab user and group
RUN groupadd -r schwab && useradd --no-log-init -r -g schwab schwab

WORKDIR /app

# Copy only the compiled binaries from builder stage (now with fixed interpreter paths)
COPY --from=builder /app/target/release/main ./
COPY --from=builder /app/migrations ./migrations

# Set proper ownership and permissions
RUN chown -R schwab:schwab /app

# Switch to non-root user
USER schwab

ENTRYPOINT ["./main"]
