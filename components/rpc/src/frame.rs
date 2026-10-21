use std::io;

use bytes::{BufMut, Bytes, BytesMut};
use flatbuffers::root;
use protocol::{AppendRequest, AppendResponse, SealRequest, SealResponse};
use snafu::{Location, ResultExt, Snafu};
use snafu_virtstack::stack_trace_debug;
use tokio_util::codec::{Decoder, Encoder};

use crate::opcode::OpCodeError;
use crate::{Header, OpCode};

const MAX_FRAME_LENGTH: usize = 16 * 1024 * 1024;

#[derive(Snafu)]
#[stack_trace_debug]
enum HeaderError {
    #[snafu(display("invalid {opcode:?} RPC header: {error}"))]
    Invalid {
        opcode: OpCode,
        #[snafu(source)]
        error: flatbuffers::InvalidFlatbuffer,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("reserved RPC opcode"))]
    Reserved {
        #[snafu(implicit)]
        location: Location,
    },
}

#[derive(Snafu)]
#[stack_trace_debug]
enum CodecError {
    #[snafu(display("{reason}"))]
    InvalidInput {
        reason: &'static str,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("{reason}"))]
    InvalidData {
        reason: &'static str,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("RPC frame too large ({frame_len} > {max_frame_len})"))]
    EncodeFrameTooLarge {
        frame_len: usize,
        max_frame_len: usize,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("RPC frame too short ({frame_len} bytes)"))]
    FrameTooShort {
        frame_len: usize,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("RPC frame too large ({frame_len} > {max_frame_len})"))]
    DecodeFrameTooLarge {
        frame_len: usize,
        max_frame_len: usize,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("RPC header exceeds frame ({header_len} > {max_header_len})"))]
    HeaderExceedsFrame {
        header_len: usize,
        max_header_len: usize,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("RPC checksum mismatch (expected {expected:#010x}, got {actual:#010x})"))]
    ChecksumMismatch {
        expected: u32,
        actual: u32,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("failed to decode RPC opcode: {source}"))]
    DecodeOpcode {
        source: OpCodeError,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("failed to decode RPC header: {source}"))]
    DecodeHeader {
        source: HeaderError,
        #[snafu(implicit)]
        location: Location,
    },
}

impl CodecError {
    fn into_io(self) -> io::Error {
        let kind = match self {
            Self::InvalidInput { .. } | Self::EncodeFrameTooLarge { .. } => {
                io::ErrorKind::InvalidInput
            }
            _ => io::ErrorKind::InvalidData,
        };
        io::Error::new(kind, self)
    }
}

/// [frame-length:4][header-length:4][opcode:1][flatbuffers][payload][checksum:4]
pub struct Frame {
    opcode: OpCode,
    header: Bytes,
    payload: Option<Bytes>,
}

impl Frame {
    pub fn new(opcode: OpCode, header: Bytes, payload: Option<Bytes>) -> io::Result<Self> {
        Self::try_new(opcode, header, payload)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    fn try_new(opcode: OpCode, header: Bytes, payload: Option<Bytes>) -> Result<Self, HeaderError> {
        parse_header(opcode, &header)?;
        Ok(Self {
            opcode,
            header,
            payload,
        })
    }

    pub fn opcode(&self) -> OpCode {
        self.opcode
    }

    pub fn header(&self) -> Header<'_> {
        parse_header(self.opcode, &self.header).expect("frame header was verified on construction")
    }

    pub fn payload(&self) -> Option<&Bytes> {
        self.payload.as_ref()
    }
}

fn parse_header<'a>(opcode: OpCode, bytes: &'a [u8]) -> Result<Header<'a>, HeaderError> {
    match opcode {
        OpCode::AppendRequest => root::<AppendRequest>(bytes)
            .map(Header::AppendRequest)
            .context(InvalidSnafu { opcode }),
        OpCode::SealRequest => root::<SealRequest>(bytes)
            .map(Header::SealRequest)
            .context(InvalidSnafu { opcode }),
        OpCode::AppendResponse => root::<AppendResponse>(bytes)
            .map(Header::AppendResponse)
            .context(InvalidSnafu { opcode }),
        OpCode::SealResponse => root::<SealResponse>(bytes)
            .map(Header::SealResponse)
            .context(InvalidSnafu { opcode }),
        OpCode::Unspecified => Err(ReservedSnafu.build()),
    }
}

/// Incremental length-field codec. Limits count bytes after the frame-length field.
pub struct FrameCodec {
    max_frame_len: usize,
}

impl FrameCodec {
    pub fn new(max_frame_len: usize) -> Self {
        Self { max_frame_len }
    }
}

impl Default for FrameCodec {
    fn default() -> Self {
        Self::new(MAX_FRAME_LENGTH)
    }
}

impl Encoder<Frame> for FrameCodec {
    type Error = io::Error;

    fn encode(&mut self, frame: Frame, dst: &mut BytesMut) -> io::Result<()> {
        let payload_len = frame.payload.as_ref().map_or(0, Bytes::len);
        let frame_len = frame
            .header
            .len()
            .checked_add(payload_len)
            .and_then(|len| len.checked_add(9))
            .ok_or_else(|| {
                InvalidInputSnafu {
                    reason: "RPC frame length overflow",
                }
                .build()
                .into_io()
            })?;
        if frame_len > self.max_frame_len {
            return Err(EncodeFrameTooLargeSnafu {
                frame_len,
                max_frame_len: self.max_frame_len,
            }
            .build()
            .into_io());
        }
        let frame_len = u32::try_from(frame_len).map_err(|_| {
            InvalidInputSnafu {
                reason: "RPC frame length overflow",
            }
            .build()
            .into_io()
        })?;
        let header_len = u32::try_from(frame.header.len()).map_err(|_| {
            InvalidInputSnafu {
                reason: "RPC header length overflow",
            }
            .build()
            .into_io()
        })?;

        let total_len = (frame_len as usize).checked_add(4).ok_or_else(|| {
            InvalidInputSnafu {
                reason: "RPC frame length overflow",
            }
            .build()
            .into_io()
        })?;
        let start = dst.len();
        dst.reserve(total_len);
        dst.put_u32(frame_len);
        dst.put_u32(header_len);
        dst.put_u8(frame.opcode as u8);
        dst.extend_from_slice(&frame.header);
        if let Some(payload) = frame.payload {
            dst.extend_from_slice(&payload);
        }
        let checksum = crc32fast::hash(&dst[start + 4..]);
        dst.put_u32(checksum);
        Ok(())
    }
}

impl Decoder for FrameCodec {
    type Item = Frame;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> io::Result<Option<Frame>> {
        if src.len() < 4 {
            return Ok(None);
        }
        let frame_len = u32::from_be_bytes(src[..4].try_into().unwrap()) as usize;
        if frame_len < 9 {
            return Err(FrameTooShortSnafu { frame_len }.build().into_io());
        }
        if frame_len > self.max_frame_len {
            return Err(DecodeFrameTooLargeSnafu {
                frame_len,
                max_frame_len: self.max_frame_len,
            }
            .build()
            .into_io());
        }
        let total_len = frame_len.checked_add(4).ok_or_else(|| {
            InvalidDataSnafu {
                reason: "RPC frame length overflow",
            }
            .build()
            .into_io()
        })?;
        if src.len() < 8 {
            return Ok(None);
        }
        let header_len = u32::from_be_bytes(src[4..8].try_into().unwrap()) as usize;
        if header_len > frame_len - 9 {
            return Err(HeaderExceedsFrameSnafu {
                header_len,
                max_header_len: frame_len - 9,
            }
            .build()
            .into_io());
        }
        if src.len() < total_len {
            return Ok(None);
        }
        let opcode = OpCode::parse(src[8])
            .context(DecodeOpcodeSnafu)
            .map_err(CodecError::into_io)?;
        let actual = u32::from_be_bytes(src[total_len - 4..total_len].try_into().unwrap());
        let expected = crc32fast::hash(&src[4..total_len - 4]);
        if actual != expected {
            return Err(ChecksumMismatchSnafu { expected, actual }.build().into_io());
        }

        let wire = src.split_to(total_len).freeze();
        let header = wire.slice(9..9 + header_len);
        let payload_start = 9 + header_len;
        let payload =
            (payload_start < total_len - 4).then(|| wire.slice(payload_start..total_len - 4));
        Frame::try_new(opcode, header, payload)
            .context(DecodeHeaderSnafu)
            .map(Some)
            .map_err(CodecError::into_io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flatbuffers::FlatBufferBuilder;
    use protocol::{
        AppendRequest, AppendRequestArgs, AppendResponse, AppendResponseArgs, SealRequest,
        SealRequestArgs, SealResponse, SealResponseArgs,
    };
    use tokio_util::codec::{Decoder, Encoder};

    fn header_bytes(opcode: OpCode) -> Bytes {
        let mut builder = FlatBufferBuilder::new();
        match opcode {
            OpCode::AppendRequest => {
                let root = AppendRequest::create(
                    &mut builder,
                    &AppendRequestArgs {
                        request_id: 11,
                        ..Default::default()
                    },
                );
                builder.finish(root, None);
            }
            OpCode::SealRequest => {
                let root = SealRequest::create(
                    &mut builder,
                    &SealRequestArgs {
                        request_id: 12,
                        ..Default::default()
                    },
                );
                builder.finish(root, None);
            }
            OpCode::AppendResponse => {
                let root = AppendResponse::create(
                    &mut builder,
                    &AppendResponseArgs {
                        request_id: 13,
                        ..Default::default()
                    },
                );
                builder.finish(root, None);
            }
            OpCode::SealResponse => {
                let root = SealResponse::create(
                    &mut builder,
                    &SealResponseArgs {
                        request_id: 14,
                        ..Default::default()
                    },
                );
                builder.finish(root, None);
            }
            OpCode::Unspecified => panic!("reserved opcode"),
        }
        Bytes::copy_from_slice(builder.finished_data())
    }

    fn encoded_request() -> bytes::BytesMut {
        let frame = Frame::new(
            OpCode::AppendRequest,
            header_bytes(OpCode::AppendRequest),
            Some(Bytes::from_static(b"data")),
        )
        .unwrap();
        let mut wire = bytes::BytesMut::new();
        FrameCodec::new(1024).encode(frame, &mut wire).unwrap();
        wire
    }

    #[test]
    fn decoder_rejects_short_and_oversized_length_prefixes() {
        for length in [8u32, 1025] {
            let mut wire = bytes::BytesMut::from(&length.to_be_bytes()[..]);
            let error = FrameCodec::new(1024).decode(&mut wire).err().unwrap();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        }
    }

    #[test]
    fn decoder_rejects_unknown_opcode_even_with_valid_checksum() {
        let mut wire = encoded_request();
        wire[8] = 0;
        let checksum = crc32fast::hash(&wire[4..wire.len() - 4]);
        let end = wire.len();
        wire[end - 4..].copy_from_slice(&checksum.to_be_bytes());
        let error = FrameCodec::new(1024).decode(&mut wire).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("unknown RPC opcode 0"));
        let diagnostic = error
            .get_ref()
            .unwrap()
            .downcast_ref::<CodecError>()
            .unwrap();
        assert!(matches!(
            diagnostic,
            CodecError::DecodeOpcode {
                source: OpCodeError::Unknown { value: 0, .. },
                ..
            }
        ));
        let stack = format!("{diagnostic:?}");
        assert!(stack.starts_with(
            "0: failed to decode RPC opcode: unknown RPC opcode 0, at components/rpc/src/frame.rs:"
        ));
        assert!(stack.contains("\n1: unknown RPC opcode 0, at components/rpc/src/opcode.rs:"));
        assert!(snafu_virtstack::StackError::next(diagnostic).is_some());
    }

    #[test]
    fn decoder_rejects_corrupted_payload_checksum() {
        let mut wire = encoded_request();
        let payload_start = wire.len() - 5;
        wire[payload_start] ^= 1;
        let error = FrameCodec::new(1024).decode(&mut wire).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("checksum"));
        let stack = format!("{:?}", error.get_ref().unwrap());
        assert!(stack.starts_with("0: RPC checksum mismatch (expected 0x"));
        assert!(stack.contains(", at components/rpc/src/frame.rs:"));
    }

    #[test]
    fn decoder_rejects_invalid_flatbuffer_with_valid_checksum() {
        let mut wire = encoded_request();
        wire[9..13].copy_from_slice(&u32::MAX.to_le_bytes());
        let checksum = crc32fast::hash(&wire[4..wire.len() - 4]);
        let end = wire.len();
        wire[end - 4..].copy_from_slice(&checksum.to_be_bytes());
        let error = FrameCodec::new(1024).decode(&mut wire).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(!error.to_string().contains("checksum"));
        assert!(
            error
                .to_string()
                .contains("invalid AppendRequest RPC header")
        );
        let diagnostic = error
            .get_ref()
            .unwrap()
            .downcast_ref::<CodecError>()
            .unwrap();
        assert!(matches!(
            diagnostic,
            CodecError::DecodeHeader {
                source: HeaderError::Invalid {
                    opcode: OpCode::AppendRequest,
                    ..
                },
                ..
            }
        ));
        let stack = format!("{diagnostic:?}");
        assert!(
            stack.starts_with("0: failed to decode RPC header: invalid AppendRequest RPC header: ")
        );
        assert!(stack.contains(", at components/rpc/src/frame.rs:"));
        assert!(stack.contains("\n1: invalid AppendRequest RPC header: "));
        assert!(stack.contains("\n2: "));
        assert!(format!("{error:?}").contains("0: failed to decode RPC header"));
        let source = std::error::Error::source(diagnostic).unwrap();
        let verifier = std::error::Error::source(source).unwrap();
        assert!(error.to_string().contains(&verifier.to_string()));
    }

    #[test]
    fn encoder_rejects_oversize_without_mutating_output() {
        let frame = Frame::new(
            OpCode::AppendRequest,
            header_bytes(OpCode::AppendRequest),
            None,
        )
        .unwrap();
        let mut output = bytes::BytesMut::from(&b"existing"[..]);
        let error = FrameCodec::new(9).encode(frame, &mut output).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(&output[..], b"existing");
        let stack = format!("{:?}", error.get_ref().unwrap());
        assert!(stack.starts_with("0: RPC frame too large ("));
        assert!(stack.contains(", at components/rpc/src/frame.rs:"));
    }

    #[test]
    fn decoder_rejects_truncated_frame_at_eof() {
        let mut wire = encoded_request();
        wire.truncate(wire.len() - 1);
        assert_eq!(
            FrameCodec::new(1024)
                .decode_eof(&mut wire)
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::Other
        );
    }

    #[test]
    fn decoder_rejects_impossible_header_length_before_buffering_body() {
        let mut wire = bytes::BytesMut::from(&[0, 0, 0, 9, 0, 0, 0, 10][..]);
        let error = FrameCodec::new(1024).decode(&mut wire).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn frame_and_codec_are_available_from_crate_root() {
        let _codec = crate::FrameCodec::default();
        let _frame = crate::Frame::new(
            OpCode::AppendRequest,
            header_bytes(OpCode::AppendRequest),
            None,
        )
        .unwrap();
    }

    #[test]
    fn ieee_crc32_matches_standard_test_vector() {
        assert_eq!(crc32fast::hash(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn encoder_writes_big_endian_lengths_opcode_payload_and_ieee_crc32() {
        let header = header_bytes(OpCode::AppendRequest);
        let header_len = header.len();
        let frame = Frame::new(
            OpCode::AppendRequest,
            header.clone(),
            Some(Bytes::from_static(b"data")),
        )
        .unwrap();
        let mut wire = bytes::BytesMut::new();
        FrameCodec::new(1024).encode(frame, &mut wire).unwrap();

        assert_eq!(
            u32::from_be_bytes(wire[..4].try_into().unwrap()) as usize,
            wire.len() - 4
        );
        assert_eq!(
            u32::from_be_bytes(wire[4..8].try_into().unwrap()) as usize,
            header_len
        );
        assert_eq!(wire[8], 1);
        assert_eq!(&wire[9..9 + header_len], &header[..]);
        assert_eq!(&wire[9 + header_len..wire.len() - 4], b"data");
        assert_eq!(
            u32::from_be_bytes(wire[wire.len() - 4..].try_into().unwrap()),
            crc32fast::hash(&wire[4..wire.len() - 4])
        );
    }

    #[test]
    fn decoder_waits_for_complete_frame() {
        let frame = Frame::new(
            OpCode::AppendRequest,
            header_bytes(OpCode::AppendRequest),
            Some(Bytes::from_static(b"data")),
        )
        .unwrap();
        let mut wire = bytes::BytesMut::new();
        FrameCodec::new(1024).encode(frame, &mut wire).unwrap();
        let mut input = bytes::BytesMut::new();
        let mut codec = FrameCodec::new(1024);

        input.extend_from_slice(&wire[..3]);
        assert!(codec.decode(&mut input).unwrap().is_none());
        input.extend_from_slice(&wire[3..wire.len() - 1]);
        assert!(codec.decode(&mut input).unwrap().is_none());
        input.extend_from_slice(&wire[wire.len() - 1..]);
        let decoded = codec.decode(&mut input).unwrap().unwrap();
        assert_eq!(decoded.opcode(), OpCode::AppendRequest);
        assert!(
            matches!(decoded.header(), Header::AppendRequest(request) if request.request_id() == 11)
        );
        assert_eq!(decoded.payload().unwrap().as_ref(), b"data");
        assert!(input.is_empty());
    }

    #[test]
    fn decoder_reads_adjacent_frames_of_all_four_types() {
        let mut codec = FrameCodec::new(1024);
        let mut wire = bytes::BytesMut::new();
        let opcodes = [
            OpCode::AppendRequest,
            OpCode::AppendResponse,
            OpCode::SealRequest,
            OpCode::SealResponse,
        ];
        for opcode in opcodes {
            codec
                .encode(
                    Frame::new(opcode, header_bytes(opcode), None).unwrap(),
                    &mut wire,
                )
                .unwrap();
        }
        for opcode in opcodes {
            let frame = codec.decode(&mut wire).unwrap().unwrap();
            assert_eq!(frame.opcode(), opcode);
            assert!(frame.payload().is_none());
        }
        assert!(codec.decode(&mut wire).unwrap().is_none());
        assert!(wire.is_empty());
    }

    #[test]
    fn frame_exposes_verified_header_and_payload() {
        let payload = Bytes::from_static(b"hello");
        let frame = Frame::new(
            OpCode::AppendRequest,
            header_bytes(OpCode::AppendRequest),
            Some(payload.clone()),
        )
        .unwrap();
        assert_eq!(frame.opcode(), OpCode::AppendRequest);
        assert!(
            matches!(frame.header(), Header::AppendRequest(request) if request.request_id() == 11)
        );
        assert_eq!(frame.payload(), Some(&payload));
    }

    #[test]
    fn frame_exposes_all_response_and_request_headers() {
        let seal_request =
            Frame::new(OpCode::SealRequest, header_bytes(OpCode::SealRequest), None).unwrap();
        assert!(
            matches!(seal_request.header(), Header::SealRequest(request) if request.request_id() == 12)
        );
        assert_eq!(seal_request.payload(), None);

        let append_response = Frame::new(
            OpCode::AppendResponse,
            header_bytes(OpCode::AppendResponse),
            None,
        )
        .unwrap();
        assert!(
            matches!(append_response.header(), Header::AppendResponse(response) if response.request_id() == 13)
        );

        let seal_response = Frame::new(
            OpCode::SealResponse,
            header_bytes(OpCode::SealResponse),
            None,
        )
        .unwrap();
        assert!(
            matches!(seal_response.header(), Header::SealResponse(response) if response.request_id() == 14)
        );
    }

    #[test]
    fn frame_rejects_invalid_flatbuffers_and_reserved_opcode() {
        let error = Frame::new(OpCode::AppendRequest, Bytes::from_static(b"invalid"), None)
            .err()
            .unwrap();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        let diagnostic = error
            .get_ref()
            .unwrap()
            .downcast_ref::<HeaderError>()
            .unwrap();
        let stack = format!("{diagnostic:?}");
        assert!(stack.starts_with("0: invalid AppendRequest RPC header: "));
        assert!(stack.contains(", at components/rpc/src/frame.rs:"));
        assert!(stack.contains("\n1: "));
        assert!(std::error::Error::source(diagnostic).is_some());

        let error = Frame::new(
            OpCode::Unspecified,
            header_bytes(OpCode::AppendRequest),
            None,
        )
        .err()
        .unwrap();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        let stack = format!("{:?}", error.get_ref().unwrap());
        assert!(stack.starts_with("0: reserved RPC opcode, at components/rpc/src/frame.rs:"));
    }
}
