use std::io;

use bytes::{BufMut, Bytes, BytesMut};
use flatbuffers::root;
use protocol::{AppendRequest, AppendResponse, SealRequest, SealResponse};
use tokio_util::codec::{Decoder, Encoder};

use crate::{Header, OpCode};

const MAX_FRAME_LENGTH: usize = 16 * 1024 * 1024;

/// [frame-length:4][header-length:4][opcode:1][flatbuffers][payload][checksum:4]
pub struct Frame {
    opcode: OpCode,
    header: Bytes,
    payload: Option<Bytes>,
}

impl Frame {
    pub fn new(opcode: OpCode, header: Bytes, payload: Option<Bytes>) -> io::Result<Self> {
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

fn parse_header<'a>(opcode: OpCode, bytes: &'a [u8]) -> io::Result<Header<'a>> {
    let invalid = |error| io::Error::new(io::ErrorKind::InvalidData, error);
    match opcode {
        OpCode::AppendRequest => root::<AppendRequest>(bytes)
            .map(Header::AppendRequest)
            .map_err(invalid),
        OpCode::SealRequest => root::<SealRequest>(bytes)
            .map(Header::SealRequest)
            .map_err(invalid),
        OpCode::AppendResponse => root::<AppendResponse>(bytes)
            .map(Header::AppendResponse)
            .map_err(invalid),
        OpCode::SealResponse => root::<SealResponse>(bytes)
            .map(Header::SealResponse)
            .map_err(invalid),
        OpCode::Unspecified => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "reserved RPC opcode",
        )),
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
                io::Error::new(io::ErrorKind::InvalidInput, "RPC frame length overflow")
            })?;
        if frame_len > self.max_frame_len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "RPC frame too large",
            ));
        }
        let frame_len = u32::try_from(frame_len).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "RPC frame length overflow")
        })?;
        let header_len = u32::try_from(frame.header.len()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "RPC header length overflow")
        })?;

        let total_len = (frame_len as usize).checked_add(4).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "RPC frame length overflow")
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
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "RPC frame too short",
            ));
        }
        if frame_len > self.max_frame_len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "RPC frame too large",
            ));
        }
        let total_len = frame_len.checked_add(4).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "RPC frame length overflow")
        })?;
        if src.len() < 8 {
            return Ok(None);
        }
        let header_len = u32::from_be_bytes(src[4..8].try_into().unwrap()) as usize;
        if header_len > frame_len - 9 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "RPC header exceeds frame",
            ));
        }
        if src.len() < total_len {
            return Ok(None);
        }
        let opcode = OpCode::try_from(src[8])?;
        let checksum = u32::from_be_bytes(src[total_len - 4..total_len].try_into().unwrap());
        if checksum != crc32fast::hash(&src[4..total_len - 4]) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "RPC checksum mismatch",
            ));
        }

        let wire = src.split_to(total_len).freeze();
        let header = wire.slice(9..9 + header_len);
        let payload_start = 9 + header_len;
        let payload =
            (payload_start < total_len - 4).then(|| wire.slice(payload_start..total_len - 4));
        Frame::new(opcode, header, payload).map(Some)
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
        assert!(error.to_string().contains("opcode"));
    }

    #[test]
    fn decoder_rejects_corrupted_payload_checksum() {
        let mut wire = encoded_request();
        let payload_start = wire.len() - 5;
        wire[payload_start] ^= 1;
        let error = FrameCodec::new(1024).decode(&mut wire).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("checksum"));
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
        assert!(Frame::new(OpCode::AppendRequest, Bytes::from_static(b"invalid"), None).is_err());
        assert!(
            Frame::new(
                OpCode::Unspecified,
                header_bytes(OpCode::AppendRequest),
                None
            )
            .is_err()
        );
    }
}
