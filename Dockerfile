# Not compatible with signal-cli
#FROM alpine:latest
#COPY target/x86_64-unknown-linux-musl/release/signal-pager ./

FROM ghcr.io/dadevel/debian:latest
COPY target/release/signal-pager ./
COPY signal-cli ./
