use bytes::Bytes;

use crate::Header;
use crate::OpCode;

pub struct Frame<'a> {
    opcode: OpCode,
    header: Header<'a>,
    payload: Option<Bytes>,
}
