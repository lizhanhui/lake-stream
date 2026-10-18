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

## RPC frame layout

`[frame-length:4][header-length:4][opcode:1][flatbuffers][payload][checksum:4]`

Both lengths and the checksum are unsigned 32-bit big-endian integers.
`frame-length` counts the bytes after itself, including the checksum.
`header-length` counts only the FlatBuffers root bytes (not the opcode or payload).
The four opcode values are append request = 1, seal request = 2, append response = 3,
and seal response = 4. Value 0 is reserved. The payload is opaque and may be
empty; an empty payload is decoded as `None`. The checksum is CRC-32/IEEE,
computed by `crc32fast` over the bytes starting with `header-length` and ending
with the payload; it excludes `frame-length` and the checksum itself. This is
not CRC32C: frames using the former CRC32C checksum are incompatible.

The `rpc::FrameCodec` encoder and decoder can be used with
`tokio_util::codec::Framed`. `FrameCodec::default()` limits `frame-length` to
16 MiB; `FrameCodec::new(max_frame_len)` sets a different limit. Headers are
verified as the FlatBuffers root type selected by the opcode. The schema has no
per-table identifier, so verification checks structural validity, not the
original table type independently of the opcode.
