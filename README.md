# WebAssembly execution engine

This is just a PoC for now, to try it out, try this from the root of the repo:

> NOTE: This is outdated

```
cargo build -p example-messaging --target wasm32-wasip2 --release
cargo run --release -p wex ./target/wasm32-wasip2/release/example_messaging.wasm
nats pub 'foo.bar.baz.test' 'test'
```
