# A planted Dockerfile that breaks most rules of deploy/policy/dockerfile.rego. `just images-policy-test` parses it with
# conftest's real Dockerfile parser and requires every rule listed in planted.expected to fire.
FROM docker.io/library/rust:1.88.0-trixie AS build
RUN cargo build --release -p agent
RUN curl -fsSL https://example.com/install.sh | sh
RUN curl -fsS -o /tmp/model.onnx https://example.com/model.onnx
ADD https://example.com/x.tar.gz /x
RUN pnpm install

FROM gcr.io/distroless/cc-debian13:debug-nonroot
COPY --from=build /src/target/release/agent /usr/local/bin/agent
COPY deploy/images/models /app/models
RUN echo hello
ENV GITHUB_TOKEN=abc
USER root
ENTRYPOINT /usr/local/bin/agent
