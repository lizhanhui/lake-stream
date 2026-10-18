use bytes::Bytes;
use criterion::measurement::WallTime;
use criterion::{
    BenchmarkGroup, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main,
};
use flatbuffers::FlatBufferBuilder;
use futures_util::{SinkExt, StreamExt};
use protocol::{
    AppendRequest, AppendRequestArgs, AppendResponse, AppendResponseArgs, Extent, Status,
    StatusArgs, Stream,
};
use rpc::{Frame, FrameCodec, Header, OpCode};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::Framed;

fn request_header() -> Bytes {
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

fn response_header() -> Bytes {
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

async fn start_server(
    payload: Bytes,
    max_frame_len: usize,
) -> (Framed<TcpStream, FrameCodec>, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (server, _) = listener.accept().await.unwrap();
    client.set_nodelay(true).unwrap();
    server.set_nodelay(true).unwrap();

    let server = tokio::spawn(async move {
        let mut connection = Framed::new(server, FrameCodec::new(max_frame_len));
        let mut first_request = true;
        while let Some(frame) = connection.next().await {
            let frame = frame.unwrap();
            assert_eq!(frame.opcode(), OpCode::AppendRequest);
            assert!(
                matches!(frame.header(), Header::AppendRequest(header) if header.request_id() == 11)
            );
            assert_eq!(frame.payload().map_or(0, Bytes::len), payload.len());
            if first_request {
                assert_eq!(
                    frame.payload().map(Bytes::as_ref).unwrap_or(&[]),
                    payload.as_ref()
                );
                first_request = false;
            }
            connection
                .send(Frame::new(OpCode::AppendResponse, response_header(), None).unwrap())
                .await
                .unwrap();
        }
    });
    (Framed::new(client, FrameCodec::new(max_frame_len)), server)
}

async fn ping_pong(client: &mut Framed<TcpStream, FrameCodec>, payload: &Bytes) -> u64 {
    let body = (!payload.is_empty()).then(|| payload.clone());
    client
        .send(Frame::new(OpCode::AppendRequest, request_header(), body).unwrap())
        .await
        .unwrap();
    let frame = client
        .next()
        .await
        .expect("server closed connection")
        .unwrap();
    assert_eq!(frame.opcode(), OpCode::AppendResponse);
    assert!(frame.payload().is_none());
    match frame.header() {
        Header::AppendResponse(response) => {
            assert_eq!(response.request_id(), 11);
            response.offset()
        }
        _ => panic!("expected append response"),
    }
}

fn benchmark_round_trip(
    group: &mut BenchmarkGroup<'_, WallTime>,
    runtime: &tokio::runtime::Runtime,
    payload_len: usize,
) {
    let payload = Bytes::from(
        (0..payload_len)
            .map(|i| (i % 251) as u8)
            .collect::<Vec<_>>(),
    );
    let request_len = request_header().len() + payload_len + 13;
    let response_len = response_header().len() + 13;
    let max_frame_len = request_len.max(response_len) - 4;
    let (mut client, server) = runtime.block_on(start_server(payload.clone(), max_frame_len));
    for _ in 0..2 {
        assert_eq!(runtime.block_on(ping_pong(&mut client, &payload)), 1024);
    }

    group.throughput(Throughput::Bytes((request_len + response_len) as u64));
    group.bench_with_input(
        BenchmarkId::from_parameter(payload_len),
        &payload,
        |b, payload| {
            b.iter(|| std::hint::black_box(runtime.block_on(ping_pong(&mut client, payload))));
        },
    );

    drop(client);
    runtime.block_on(server).unwrap();
}

fn benchmarks(c: &mut Criterion) {
    let request_header_len = request_header().len();
    let max_payload_len = u32::MAX as usize - request_header_len - 9;
    let sizes = std::env::var("RPC_BENCH_PAYLOAD_SIZES")
        .unwrap_or_else(|_| "0,1024,65536,1048576".to_string());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let mut group = c.benchmark_group("append_ping_pong_tcp");
    for size in sizes.split(',') {
        let size: usize = size
            .trim()
            .parse()
            .expect("invalid RPC_BENCH_PAYLOAD_SIZES");
        assert!(size <= max_payload_len, "payload exceeds RPC frame limit");
        benchmark_round_trip(&mut group, &runtime, size);
    }
    group.finish();
}

criterion_group!(benches, benchmarks);
criterion_main!(benches);
