#![doc = include_str!("../doc/protocol.md")]

#[allow(
    unused_imports,
    unsafe_op_in_unsafe_fn,
    clippy::derivable_impls,
    clippy::extra_unused_lifetimes,
    clippy::missing_safety_doc
)]
mod generated {
    include!(concat!(env!("OUT_DIR"), "/flatbuffers/mod.rs"));
}

pub use generated::stream::store::*;

#[cfg(test)]
mod tests {
    use super::*;
    use flatbuffers::{FlatBufferBuilder, root};

    #[test]
    fn append_request_round_trip() {
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

        let request = root::<AppendRequest>(builder.finished_data()).unwrap();
        assert_eq!(request.request_id(), 11);
        let extent = request.extent().unwrap();
        assert_eq!(extent.extent_id(), 7);
        assert_eq!(extent.stream().stream_id(), 42);
        assert_eq!(extent.stream().epoch(), 3);
    }

    #[test]
    fn append_response_round_trip() {
        let mut builder = FlatBufferBuilder::new();
        let stream = Stream::new(42, 3);
        let extent = Extent::new(&stream, 7);
        let message = builder.create_string("append failed");
        let status = Status::create(
            &mut builder,
            &StatusArgs {
                code: -1,
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

        let response = root::<AppendResponse>(builder.finished_data()).unwrap();
        assert_eq!(response.request_id(), 11);
        assert_eq!(response.extent().unwrap(), &extent);
        assert_eq!(response.offset(), 1024);
        let status = response.status().unwrap();
        assert_eq!(status.code(), -1);
        assert_eq!(status.message(), Some("append failed"));
    }

    #[test]
    fn seal_request_round_trip() {
        for source in [SealSource::Client, SealSource::StreamManager] {
            let mut builder = FlatBufferBuilder::new();
            let stream = Stream::new(42, 3);
            let request = SealRequest::create(
                &mut builder,
                &SealRequestArgs {
                    request_id: 12,
                    stream: Some(&stream),
                    source,
                },
            );
            builder.finish(request, None);

            let request = root::<SealRequest>(builder.finished_data()).unwrap();
            assert_eq!(request.request_id(), 12);
            assert_eq!(request.stream().unwrap(), &stream);
            assert_eq!(request.source(), source);
        }
    }

    #[test]
    fn seal_request_defaults_to_client() {
        let mut builder = FlatBufferBuilder::new();
        let request = SealRequest::create(&mut builder, &SealRequestArgs::default());
        builder.finish(request, None);

        let request = root::<SealRequest>(builder.finished_data()).unwrap();
        assert_eq!(request.source(), SealSource::Client);
    }

    #[test]
    fn seal_response_round_trip() {
        let mut builder = FlatBufferBuilder::new();
        let stream = Stream::new(42, 3);
        let extent = Extent::new(&stream, 7);
        let message = builder.create_string("sealed");
        let status = Status::create(
            &mut builder,
            &StatusArgs {
                code: 0,
                message: Some(message),
            },
        );
        let response = SealResponse::create(
            &mut builder,
            &SealResponseArgs {
                request_id: 12,
                status: Some(status),
                extent: Some(&extent),
                offset: 2048,
            },
        );
        builder.finish(response, None);

        let response = root::<SealResponse>(builder.finished_data()).unwrap();
        assert_eq!(response.request_id(), 12);
        assert_eq!(response.extent().unwrap(), &extent);
        assert_eq!(response.offset(), 2048);
        let status = response.status().unwrap();
        assert_eq!(status.code(), 0);
        assert_eq!(status.message(), Some("sealed"));
    }
}
