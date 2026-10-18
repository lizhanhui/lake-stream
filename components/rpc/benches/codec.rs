use std::hint::black_box;

use bytes::{Bytes, BytesMut};
use criterion::measurement::WallTime;
use criterion::{
    BatchSize, BenchmarkGroup, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main,
};
use flatbuffers::{FlatBufferBuilder, root};
use protocol::{
    AppendRequest, AppendRequestArgs, AppendResponse, AppendResponseArgs, Extent, Status,
    StatusArgs, Stream,
};
use rpc::{Frame, FrameCodec, Header, OpCode};
use tokio_util::codec::{Decoder, Encoder};

fn append_request_header() -> Bytes {
    let mut builder = FlatBufferBuilder::new();
    let stream = Stream::new(42, 3);
    let extent = Extent::new(&stream, 7);
    let request = AppendRequest::create(
        &mut builder,
        &AppendRequestArgs {
            request_id: 11,
            extent: Some(&extent),
        },
    );
    builder.finish(request, None);
    Bytes::copy_from_slice(builder.finished_data())
}

fn append_response_header() -> Bytes {
    let mut builder = FlatBufferBuilder::new();
    let stream = Stream::new(42, 3);
    let extent = Extent::new(&stream, 7);
    let message = builder.create_string("ok");
    let status = Status::create(
        &mut builder,
        &StatusArgs {
            code: 0,
            message: Some(message),
        },
    );
    let response = AppendResponse::create(
        &mut builder,
        &AppendResponseArgs {
            request_id: 11,
            extent: Some(&extent),
            status: Some(status),
            offset: 1024,
        },
    );
    builder.finish(response, None);
    Bytes::copy_from_slice(builder.finished_data())
}

fn payload_sizes(max_payload_len: usize) -> Vec<usize> {
    let input = std::env::var("RPC_BENCH_PAYLOAD_SIZES")
        .unwrap_or_else(|_| "0,1024,65536,1048576".to_string());
    input
        .split(',')
        .map(|size| {
            let size = size
                .trim()
                .parse()
                .expect("invalid RPC_BENCH_PAYLOAD_SIZES");
            assert!(size <= max_payload_len, "payload exceeds RPC frame limit");
            size
        })
        .collect()
}

fn benchmark_headers(c: &mut Criterion) {
    let request = append_request_header();
    let response = append_response_header();
    assert_eq!(root::<AppendRequest>(&request).unwrap().request_id(), 11);
    assert_eq!(root::<AppendResponse>(&response).unwrap().offset(), 1024);

    let mut group = c.benchmark_group("append_request/header");
    group.bench_function("serialize", |b| {
        b.iter(|| black_box(append_request_header()));
    });
    group.bench_function("verify", |b| {
        b.iter(|| {
            let header = root::<AppendRequest>(black_box(request.as_ref())).unwrap();
            black_box(header.request_id())
        });
    });
    group.finish();

    let mut group = c.benchmark_group("append_response/header");
    group.bench_function("serialize", |b| {
        b.iter(|| black_box(append_response_header()));
    });
    group.bench_function("verify", |b| {
        b.iter(|| {
            let header = root::<AppendResponse>(black_box(response.as_ref())).unwrap();
            black_box(header.offset())
        });
    });
    group.finish();
}

fn benchmark_frame(
    group: &mut BenchmarkGroup<'_, WallTime>,
    opcode: OpCode,
    header: Bytes,
    payload: Option<Bytes>,
) {
    let size = payload.as_ref().map_or(0, Bytes::len);
    let frame_len = header
        .len()
        .checked_add(size)
        .and_then(|n| n.checked_add(9))
        .expect("payload too large");
    assert!(
        u32::try_from(frame_len).is_ok(),
        "payload exceeds RPC frame limit"
    );
    let mut codec = FrameCodec::new(frame_len);
    let mut wire = BytesMut::new();
    codec
        .encode(
            Frame::new(opcode, header.clone(), payload.clone()).unwrap(),
            &mut wire,
        )
        .unwrap();
    let decoded = codec.decode(&mut wire.clone()).unwrap().unwrap();
    assert_eq!(decoded.opcode(), opcode);
    assert_eq!(
        decoded.payload().map(Bytes::as_ref),
        payload.as_ref().map(Bytes::as_ref)
    );
    match decoded.header() {
        Header::AppendRequest(request) => assert_eq!(request.request_id(), 11),
        Header::AppendResponse(response) => assert_eq!(response.offset(), 1024),
        _ => panic!("unexpected benchmark header"),
    }

    group.throughput(Throughput::Bytes(header.len() as u64));
    group.bench_function(BenchmarkId::new("frame_new", size), |b| {
        b.iter(|| black_box(Frame::new(opcode, header.clone(), payload.clone()).unwrap()));
    });
    group.throughput(Throughput::Bytes(wire.len() as u64));
    group.bench_function(BenchmarkId::new("frame_encode", size), |b| {
        b.iter_batched(
            || {
                (
                    Frame::new(opcode, header.clone(), payload.clone()).unwrap(),
                    BytesMut::with_capacity(wire.len()),
                )
            },
            |(frame, mut output)| {
                codec.encode(frame, &mut output).unwrap();
                black_box(output)
            },
            BatchSize::LargeInput,
        );
    });
    group.bench_function(BenchmarkId::new("frame_decode", size), |b| {
        b.iter_batched(
            || BytesMut::from(&wire[..]),
            |mut input| black_box(codec.decode(&mut input).unwrap().unwrap()),
            BatchSize::LargeInput,
        );
    });
}

fn benchmarks(c: &mut Criterion) {
    benchmark_headers(c);
    let request_header = append_request_header();
    let max_payload_len = (u32::MAX as usize) - request_header.len() - 9;
    let mut group = c.benchmark_group("append_request");
    for size in payload_sizes(max_payload_len) {
        let payload = Bytes::from((0..size).map(|i| (i % 251) as u8).collect::<Vec<_>>());
        let payload = (size != 0).then_some(payload);
        benchmark_frame(
            &mut group,
            OpCode::AppendRequest,
            request_header.clone(),
            payload,
        );
    }
    group.finish();

    let mut group = c.benchmark_group("append_response");
    benchmark_frame(
        &mut group,
        OpCode::AppendResponse,
        append_response_header(),
        None,
    );
    group.finish();
}

criterion_group!(benches, benchmarks);
criterion_main!(benches);
