# ---- Build stage ----
FROM rust:1.85 AS build
WORKDIR /app

# Now copy real source and build
COPY . .
RUN cargo build --release

# ---- Runtime stage (distroless) ----
FROM gcr.io/distroless/cc-debian12
COPY --from=build /app/target/release/polymind /usr/local/bin/polymind
ENTRYPOINT ["/usr/local/bin/polymind"]

