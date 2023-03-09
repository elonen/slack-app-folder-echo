FROM rust:1-bullseye

RUN cargo install cargo-deb

RUN apt-get -qy update
RUN apt-get -qy install lsb-release

RUN apt-get -qy install mingw-w64
RUN rustup target add x86_64-pc-windows-gnu

WORKDIR /root
RUN mkdir /root/OUTPUT

COPY Cargo.toml /root/
COPY src src
# RUN CARGO_UNSTABLE_SPARSE_REGISTRY=true cargo update
RUN cargo fetch

COPY . .
