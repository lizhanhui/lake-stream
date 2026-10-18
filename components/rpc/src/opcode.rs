use std::io;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum OpCode {
    Unspecified = 0,
    AppendRequest = 1,
    SealRequest = 2,
    AppendResponse = 3,
    SealResponse = 4,
}

impl TryFrom<u8> for OpCode {
    type Error = io::Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::AppendRequest),
            2 => Ok(Self::SealRequest),
            3 => Ok(Self::AppendResponse),
            4 => Ok(Self::SealResponse),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unknown RPC opcode",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::OpCode;

    #[test]
    fn four_opcodes_have_distinct_wire_values() {
        assert_eq!(OpCode::AppendRequest as u8, 1);
        assert_eq!(OpCode::SealRequest as u8, 2);
        assert_eq!(OpCode::AppendResponse as u8, 3);
        assert_eq!(OpCode::SealResponse as u8, 4);
    }

    #[test]
    fn rejects_unknown_opcodes() {
        for value in [0, 5, 255] {
            assert!(OpCode::try_from(value).is_err());
        }
    }
}
