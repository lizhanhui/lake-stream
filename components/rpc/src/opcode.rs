use std::io;

use snafu::{Location, Snafu};
use snafu_virtstack::stack_trace_debug;

#[derive(Snafu)]
#[stack_trace_debug]
pub(crate) enum OpCodeError {
    #[snafu(display("unknown RPC opcode {value}"))]
    Unknown {
        value: u8,
        #[snafu(implicit)]
        location: Location,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum OpCode {
    Unspecified = 0,
    AppendRequest = 1,
    SealRequest = 2,
    AppendResponse = 3,
    SealResponse = 4,
}

impl OpCode {
    pub(crate) fn parse(value: u8) -> Result<Self, OpCodeError> {
        match value {
            1 => Ok(Self::AppendRequest),
            2 => Ok(Self::SealRequest),
            3 => Ok(Self::AppendResponse),
            4 => Ok(Self::SealResponse),
            _ => Err(UnknownSnafu { value }.build()),
        }
    }
}

impl TryFrom<u8> for OpCode {
    type Error = io::Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Self::parse(value).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
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
            let error = OpCode::try_from(value).err().unwrap();
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
            let diagnostic = error
                .get_ref()
                .unwrap()
                .downcast_ref::<super::OpCodeError>()
                .unwrap();
            assert!(format!("{diagnostic:?}").starts_with(&format!(
                "0: unknown RPC opcode {value}, at components/rpc/src/opcode.rs:"
            )));
        }
    }
}
