# minimal credential service

Distributes limited pools of credentials across many applications.

**Features:**

* REST-API with [long polling](https://en.wikipedia.org/wiki/Push_technology#Long_polling) for requesting access to credentials.
* Simple configuration via one YAML file (see [example.config.yaml](example.config.yaml))
* Leased credentials expire after a configurable amount of seconds
* Native SSL
* Mandatory token authentication
* Easy to deploy - its just one binary (but it is dynamically linked)
* Usage gets equally distributed over all credentials of a service

**Missing features:**

* Lease persistence

**Why this implementation?**

* [Vault](https://www.vaultproject.io) does not seem to support a limited credential pool.

## Usage ##

Logging can be activated using the `RUST_LOG` environment variable. The levels are `debug`, `error`, `info`,
`warn`, or `trace`. See the [env_logger docs](https://docs.rs/env_logger/0.6.1/env_logger/#enabling-logging).


## REST-API ##

See the either servers source, or the included [python client implementation](../python-client).