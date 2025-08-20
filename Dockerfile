FROM ubuntu:latest

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

# Build Solidity artifacts (cached if lib/ doesn't change)
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

ENTRYPOINT ["./target/release/main"]
