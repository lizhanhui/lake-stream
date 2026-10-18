use protocol::{AppendRequest, AppendResponse, SealRequest, SealResponse};

pub enum Header<'a> {
    AppendRequest(AppendRequest<'a>),
    AppendResponse(AppendResponse<'a>),
    SealRequest(SealRequest<'a>),
    SealResponse(SealResponse<'a>),
}
