# Dockerfile
FROM rust:1.75-slim as builder
WORKDIR /usr/src/app
COPY . .
# 注：实际环境中这里会执行 cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ffmpeg libssl-dev && rm -rf /var/lib/apt/lists/*
COPY --from=builder /usr/src/app/target/release/rmc-server /usr/local/bin/
CMD ["rmc-server"]
