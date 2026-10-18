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

## RPC benchmarks

Run `cargo bench -p rpc --bench codec` to measure FlatBuffers append-request
and append-response header serialization/verification, `Frame::new`, and
`FrameCodec` encoding/decoding (including CRC-32). Request payloads default to
0, 1024, 65536, and 1048576 bytes; append responses have no body. Supply
comma-separated byte sizes for other request payloads:

```sh
RPC_BENCH_PAYLOAD_SIZES=64,4096,2097152 cargo bench -p rpc --bench codec
```

Header serialization includes copying finished FlatBuffers into `Bytes`.
Codec encode/decode timings exclude creating the frame, output buffer, and input
buffer; decode starts with a complete buffered frame. The `codec` benchmark
does not include network I/O. Zero-byte request payloads decode as `None`.

Run `cargo bench -p rpc --bench ping_pong` for an end-to-end append-request /
append-response round trip over a persistent Tokio TCP connection on loopback.
The server binds an ephemeral port, decodes and checks the request, then builds
and sends a response without a payload. The timed operation includes header
construction, frame verification, CRC-32, and both directions of socket I/O;
it excludes listener, runtime, and connection setup. Throughput counts request
plus response wire bytes. Set `RPC_BENCH_PAYLOAD_SIZES` as above to benchmark
other request payload sizes (up to the 32-bit frame-length limit); run with
`-- --test` to smoke-test the network exchanges.

Run `cargo bench -p rpc --bench pipeline` for a continuously refilled,
bounded-depth append pipeline over one persistent loopback TCP connection.
Configure request payload sizes and the maximum number of in-flight requests:

```sh
RPC_BENCH_PAYLOAD_SIZES=0,1024,65536 RPC_BENCH_PIPELINE_DEPTHS=1,4,16 cargo bench -p rpc --bench pipeline
```

The defaults are payloads of 0, 1024, 65536 and 1048576 bytes and depths of
1, 4, 16 and 64. The client keeps sending as responses free slots; the server
returns a header-only append response containing the matching `request_id`.
`throughput` reports completed request-plus-response wire bytes per wall time;
`mean_latency` reports the mean per-request duration from just before sending
to receiving and verifying its response, using `request_id` to match timestamps.
These are separate Criterion runs. Latency excludes time waiting for an available
pipeline slot, while both measurements include encoding, checksums, TCP I/O and
decoding. Listener and connection setup are excluded. Each sample runs at
least two windows of requests; depth is the maximum in-flight count, not a
guarantee that this many requests are simultaneously outstanding under
write backpressure or fast responses.
