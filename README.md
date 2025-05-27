# WebAssembly execution engine

This is just a PoC for now, to try it out, try this from the root of the repo:

```
$ export DYLD_LIBRARY_PATH=target/release # on macOS
$ cargo build -p example-f1 --target wasm32-wasip2 --release
$ cargo build -p example-f2 --target wasm32-wasip2 --release
$ cargo build -p example-memdb --target wasm32-wasip2 --release
$ cargo build -p example-redis --target wasm32-wasip2 --release
$ cargo build -p example-redis-http --target wasm32-wasip2 --release
$ cargo build -p example-sockets --target wasm32-wasip2 --release
$ cargo build -p example-hello
$ cargo run -- --http-proxy 127.0.0.1:8080
$ curl -H "X-Wasmx-Id: redis-http" "localhost:8080/set?key=hello&value=world"
$ curl -H "X-Wasmx-Id: redis-http" "localhost:8080/get?key=hello"
$ curl -H "X-Wasmx-Id: redis-http" "localhost:8080/incr?key=counter"
$ curl -H "X-Wasmx-Id: redis-http" "localhost:8080/get?key=counter"
$ curl -H "X-Wasmx-Id: f1-plug" "localhost:8080"
```
