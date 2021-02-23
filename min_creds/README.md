# minimal credential service

Distributes limited pools of credentials across many applications. The general concept is providing 
floating credentials with mandatory expiration times to maximize the usage of these credentials.

It is in the responsibility of the client applications to return credentials as soon as possible after use.

**Features:**

* REST-API with [long polling](https://en.wikipedia.org/wiki/Push_technology#Long_polling) for requesting access to credentials.
* Simple configuration via one YAML file (see [example.config.yaml](example.config.yaml))
* Leased credentials expire after a configurable amount of seconds
* Native SSL
* Mandatory token authentication
* Easy to deploy - its just one binary (but it is dynamically linked)
* Usage gets equally distributed over all credentials of a service
* Lease persistence across restarts
* Waiting clients are served in the order of their requests

**Why this implementation?**

* [Vault](https://www.vaultproject.io) does not seem to support a limited credential pool.

## Building

Building this tool requires a recent version of [rust](https://www.rust-lang.org/) and cargo - these can be 
installed using the [rustup installer](https://rustup.rs/). So far this tool has been only build on linux.

To build the tool clone this repository and just use either the [cargo build](https://doc.rust-lang.org/cargo/commands/cargo-build.html) command
or directly [cargo install](https://doc.rust-lang.org/cargo/commands/cargo-install.html) which also installs the binary after
building it.

The `--release` flag takes care of building an optimized binary

```
cargo build --release
```

When using `cargo build` the artifact can be found in `target/release`.

### Static linking openssl

To use the binary on systems with a different version of libssl it can be handy to statically link
against openssl. This can be done by enabling the `openssl-static` feature:

```
cargo build --release --features openssl-static
```

## Usage

Logging can be activated using the `RUST_LOG` environment variable. The levels are `debug`, `error`, `info`,
`warn`, or `trace`. See the [env_logger docs](https://docs.rs/env_logger/0.6.1/env_logger/#enabling-logging).


## REST-API

See either the servers source, or the included [python client implementation](../python-client).