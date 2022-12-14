# Frontends
Frontends are what faces end users and emits requests to snapfaas backend.

# GitHub Webhook Server Frontend
To build the frontend
```shell
cargo build
```

To start the frontend
```shell
# RUST_LOG=debug allows messages at or above debug level to be printed
RUST_LOG=debug target/debug/webhook --listen IP:PORT --app_config app_config.yaml --snapfaas_address IP:PORT
```

# Snapfaas backend
1. To build the backend
```shell
cargo build --bin snapctr --bin firerunner
```
`snapctr` is the controller that accepts requests over TCP connections and executes requests through
forking `firerunner`.

2. To start the backend
```shell
# GITHUB_AUTH_TOKEN environment variable allows the backend to access private github resources
GITHUB_AUTH_TOKEN=YOURTOKEN RUST_LOG=debug target/debug/snapctr --config resources/example-controller-config.yaml --port PORT --mem 1024
```

3. Request and Response format
See [snapfaas/src/request.rs](/snapfaas/src/request.rs)
