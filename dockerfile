# ---- Build stage ----
FROM rust:1.85 AS build
WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y pkg-config libssl-dev cmake && rm -rf /var/lib/apt/lists/*

# Now copy real source and build
COPY . .
RUN cargo build --release

# ---- Runtime stage (distroless) ----
FROM gcr.io/distroless/cc-debian12
COPY --from=build /app/target/release/polymind /usr/local/bin/polymind
ENTRYPOINT ["/usr/local/bin/polymind"]

