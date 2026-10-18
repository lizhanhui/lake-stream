# Protocol

## Building the Rust bindings

Cargo runs `flatc` to compile `flatbuffers/proto.fbs` when building this crate.
Install a `flatc` compiler compatible with the `flatbuffers` runtime dependency
in `Cargo.toml`, and make it available on `PATH`. Alternatively, select an
executable explicitly with `FLATC`:

```sh
cargo build -p protocol
FLATC=/path/to/flatc cargo build -p protocol
cargo test -p protocol
```

Bindings are generated in Cargo's `OUT_DIR` and exported directly from
`protocol`, including `Stream`, `Extent`, `Status`, `AppendRequest`,
`AppendResponse`, `SealRequest`, and `SealResponse`. No generated files or
symlinks are written into the source tree. Schema changes and changes to
`FLATC` or `PATH` trigger regeneration; compiler failures stop the build and
report the compiler's diagnostics. If you replace or upgrade `flatc` at the
same executable path, run `cargo clean -p protocol` before rebuilding to
regenerate the bindings.

`Stream` and `Extent` are fixed-size structs. `Status` and all request/response
messages are tables. Each message can be finished and read as a FlatBuffer
root using the runtime's verified `root` API:

```rust
use flatbuffers::{FlatBufferBuilder, root};
use protocol::{AppendRequest, AppendRequestArgs, Extent, Stream};

let mut builder = FlatBufferBuilder::new();
let stream = Stream::new(42, 3);
let extent = Extent::new(&stream, 7);
let request = AppendRequest::create(
    &mut builder,
    &AppendRequestArgs { request_id: 11, extent: Some(&extent) },
);
builder.finish(request, None);

let request = root::<AppendRequest>(builder.finished_data()).unwrap();
assert_eq!(request.request_id(), 11);
assert_eq!(request.extent().unwrap().extent_id(), 7);
```

## Frame Layout

[frame-length:4][header-length:4][opcode:1][flatbuffers][payload][checksum:4]
