use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use bytes::Bytes;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use flatbuffers::FlatBufferBuilder;
use futures_util::{SinkExt, StreamExt};
use protocol::{
    AppendRequest, AppendRequestArgs, AppendResponse, AppendResponseArgs, Extent, Status,
    StatusArgs, Stream,
};
use rpc::{Frame, FrameCodec, Header, OpCode};
use tokio::net::{
    TcpListener, TcpStream,
    tcp::{OwnedReadHalf, OwnedWriteHalf},
};
use tokio::sync::mpsc;
use tokio_util::codec::{Framed, FramedRead, FramedWrite};

fn request_header(id: u64) -> Bytes {
    let mut builder = FlatBufferBuilder::new();
    let stream = Stream::new(42, 3);
    let extent = Extent::new(&stream, 7);
    let request = AppendRequest::create(
        &mut builder,
        &AppendRequestArgs {
            request_id: id,
            extent: Some(&extent),
        },
    );
    builder.finish(request, None);
    Bytes::copy_from_slice(builder.finished_data())
}

fn response_header(id: u64) -> Bytes {
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
            request_id: id,
            extent: Some(&extent),
            status: Some(status),
            offset: 1024,
        },
    );
    builder.finish(response, None);
    Bytes::copy_from_slice(builder.finished_data())
}

async fn start_server(
    payload: Bytes,
    max_frame_len: usize,
) -> (
    FramedRead<OwnedReadHalf, FrameCodec>,
    FramedWrite<OwnedWriteHalf, FrameCodec>,
    tokio::task::JoinHandle<()>,
) {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (server, _) = listener.accept().await.unwrap();
    client.set_nodelay(true).unwrap();
    server.set_nodelay(true).unwrap();

    let task = tokio::spawn(async move {
        let mut connection = Framed::new(server, FrameCodec::new(max_frame_len));
        let mut next_id = 1u64;
        while let Some(frame) = connection.next().await {
            let frame = frame.unwrap();
            assert_eq!(frame.opcode(), OpCode::AppendRequest);
            let id = match frame.header() {
                Header::AppendRequest(request) => request.request_id(),
                _ => panic!("expected append request"),
            };
            assert_eq!(id, next_id);
            next_id += 1;
            assert_eq!(frame.payload().map_or(0, Bytes::len), payload.len());
            if id == 1 {
                assert_eq!(
                    frame.payload().map(Bytes::as_ref).unwrap_or(&[]),
                    payload.as_ref()
                );
            }
            connection
                .send(Frame::new(OpCode::AppendResponse, response_header(id), None).unwrap())
                .await
                .unwrap();
        }
    });
    let (read, write) = client.into_split();
    (
        FramedRead::new(read, FrameCodec::new(max_frame_len)),
        FramedWrite::new(write, FrameCodec::new(max_frame_len)),
        task,
    )
}

fn take_sent_at(
    id: u64,
    stamps: &mut mpsc::Receiver<(u64, Instant)>,
    pending: &mut HashMap<u64, Instant>,
) -> Option<Instant> {
    while let Ok((sent_id, sent_at)) = stamps.try_recv() {
        assert!(
            pending.insert(sent_id, sent_at).is_none(),
            "duplicate request ID"
        );
    }
    pending.remove(&id)
}

// One Criterion iteration is one completed request, not one whole pipeline batch.
async fn run_pipeline(
    reader: &mut FramedRead<OwnedReadHalf, FrameCodec>,
    writer: &mut FramedWrite<OwnedWriteHalf, FrameCodec>,
    payload: &Bytes,
    depth: usize,
    count: u64,
    next_id: &mut u64,
) -> (Duration, Duration) {
    assert!(count > 0);
    let window = depth.min(usize::try_from(count).unwrap_or(usize::MAX));
    let (stamps_tx, mut stamps_rx) = mpsc::channel::<(u64, Instant)>(window);
    let (ack_tx, mut ack_rx) = mpsc::channel::<()>(window);
    let start_id = *next_id;
    *next_id = start_id.checked_add(count).expect("request ID overflow");

    let sender = async {
        for i in 0..count {
            if i >= window as u64 {
                ack_rx.recv().await.expect("receiver stopped");
            }
            let id = start_id + i;
            let body = (!payload.is_empty()).then(|| payload.clone());
            let frame = Frame::new(OpCode::AppendRequest, request_header(id), body).unwrap();
            stamps_tx.send((id, Instant::now())).await.unwrap();
            writer.send(frame).await.unwrap();
        }
    };
    let receiver = async {
        let mut total_latency = Duration::ZERO;
        let mut sent = HashMap::new();
        for i in 0..count {
            let frame = reader
                .next()
                .await
                .expect("server closed connection")
                .unwrap();
            assert_eq!(frame.opcode(), OpCode::AppendResponse);
            assert!(frame.payload().is_none());
            let id = match frame.header() {
                Header::AppendResponse(response) => {
                    assert_eq!(response.offset(), 1024);
                    response.request_id()
                }
                _ => panic!("expected append response"),
            };
            let sent_at = take_sent_at(id, &mut stamps_rx, &mut sent)
                .expect("unknown or duplicate response ID");
            total_latency += sent_at.elapsed();
            if i < count - (window as u64) {
                ack_tx.send(()).await.unwrap();
            }
        }
        assert!(sent.is_empty(), "missing responses");
        total_latency
    };
    let start = Instant::now();
    let (_, total_latency) = tokio::join!(sender, receiver);
    (start.elapsed(), total_latency)
}

fn sample_count(iters: u64, depth: usize) -> u64 {
    let full_window = depth
        .checked_mul(2)
        .and_then(|n| n.checked_add(1))
        .expect("pipeline depth too large");
    iters.max(u64::try_from(full_window).expect("pipeline depth too large"))
}

fn benchmark_cases(
    criterion: &mut Criterion,
    runtime: &tokio::runtime::Runtime,
    sizes: &[usize],
    depths: &[usize],
    latency: bool,
) {
    let label = if latency {
        "mean_latency"
    } else {
        "throughput"
    };
    let mut group = criterion.benchmark_group(format!("append_pipeline_tcp/{label}"));
    for &size in sizes {
        let payload = Bytes::from((0..size).map(|i| (i % 251) as u8).collect::<Vec<_>>());
        let request_len = request_header(1).len() + size + 13;
        let response_len = response_header(1).len() + 13;
        let max_frame_len = request_len.max(response_len) - 4;
        for &depth in depths {
            let (mut reader, mut writer, server) =
                runtime.block_on(start_server(payload.clone(), max_frame_len));
            let mut next_id = 1;
            let count = sample_count(1, depth);
            let (wall, sum) = runtime.block_on(run_pipeline(
                &mut reader,
                &mut writer,
                &payload,
                depth,
                count,
                &mut next_id,
            ));
            assert!(wall > Duration::ZERO && sum > Duration::ZERO);

            if !latency {
                group.throughput(Throughput::Bytes((request_len + response_len) as u64));
            }
            group.bench_function(
                BenchmarkId::new(format!("{size}B"), format!("{depth}inflight")),
                |b| {
                    b.iter_custom(|iters| {
                        let count = sample_count(iters, depth);
                        let (wall, total_latency) = runtime.block_on(run_pipeline(
                            &mut reader,
                            &mut writer,
                            &payload,
                            depth,
                            count,
                            &mut next_id,
                        ));
                        let scale = iters as f64 / count as f64;
                        if latency {
                            total_latency.mul_f64(scale)
                        } else {
                            wall.mul_f64(scale)
                        }
                    });
                },
            );

            drop(writer);
            drop(reader);
            runtime.block_on(server).unwrap();
        }
    }
    group.finish();
}

fn sizes_from_env(name: &str, default: &str) -> Vec<usize> {
    std::env::var(name)
        .unwrap_or_else(|_| default.to_string())
        .split(',')
        .map(|value| {
            value
                .trim()
                .parse()
                .expect("invalid pipeline benchmark size or depth")
        })
        .collect()
}

fn benchmarks(c: &mut Criterion) {
    let (tx, mut rx) = mpsc::channel(2);
    let mut pending = HashMap::new();
    let first = Instant::now();
    let second = Instant::now();
    tx.try_send((1, first)).unwrap();
    tx.try_send((2, second)).unwrap();
    assert_eq!(take_sent_at(2, &mut rx, &mut pending), Some(second));
    assert_eq!(take_sent_at(1, &mut rx, &mut pending), Some(first));
    assert_eq!(take_sent_at(999, &mut rx, &mut pending), None);
    assert_eq!(sample_count(1, 4), 9);
    assert_eq!(sample_count(20, 4), 20);
    let sizes = sizes_from_env("RPC_BENCH_PAYLOAD_SIZES", "0,1024,65536,1048576");
    let depths = sizes_from_env("RPC_BENCH_PIPELINE_DEPTHS", "1,4,16,64");
    let max_payload = u32::MAX as usize - request_header(1).len() - 9;
    assert!(
        sizes.iter().all(|&size| size <= max_payload),
        "payload exceeds RPC frame limit"
    );
    assert!(
        depths.iter().all(|&depth| depth > 0),
        "pipeline depth must be positive"
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    benchmark_cases(c, &runtime, &sizes, &depths, false);
    benchmark_cases(c, &runtime, &sizes, &depths, true);
}

criterion_group!(benches, benchmarks);
criterion_main!(benches);
