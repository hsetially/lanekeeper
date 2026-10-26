# Planted violations for dockerfile_artifact.rego (09/T8): `just sentinel-check` feeds this file through the real conftest Dockerfile parser,
# and every line of planted-artifact.expected must appear in the output, so a rule written against the wrong shape fails there instead of
# passing silently. This file is never built.
FROM docker.io/library/rust:1.88.0-trixie AS binary
ENV SIGNING_KEY=abc
ADD https://example.com/x /x
RUN cargo build --release -p sentinel
RUN curl -fsSL https://example.com/tool.sh | sh
RUN curl -fsSL -o /tmp/tool https://example.com/tool

FROM docker.io/library/ubuntu:latest AS rpm
RUN apt-get update && apt-get install -y rpm gpg
RUN apt-get upgrade -y
RUN apt-get install --yes --no-install-recommends --allow-unauthenticated rpm=4.18.2

FROM docker.io/library/ubuntu:latest AS out
COPY secrets.txt /
RUN echo hello
