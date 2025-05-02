# WebAssembly execution engine

This is just a PoC for now, to try it out, try this from the root of the repo:

```
$ cargo build -p example-f1 --target wasm32-wasip2 --release
$ cargo build -p example-f2 --target wasm32-wasip2 --release
$ cargo build -p example-memdb --target wasm32-wasip2 --release
$ cargo build -p example-redis --target wasm32-wasip2 --release
$ cargo build -p example-redis-http --target wasm32-wasip2 --release
$ cargo build -p example-sockets --target wasm32-wasip2 --release
$ cargo run
$ curl -H "X-Wex-Id: redis-http" "localhost:8080/set?key=hello&value=world"
$ curl -H "X-Wex-Id: redis-http" "localhost:8080/get?key=hello"
$ curl -H "X-Wex-Id: redis-http" "localhost:8080/incr?key=counter"
$ curl -H "X-Wex-Id: redis-http" "localhost:8080/get?key=counter"
```
