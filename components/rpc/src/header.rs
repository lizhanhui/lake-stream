use protocol::{AppendRequest, AppendResponse, SealRequest, SealResponse};

pub enum Header<'a> {
    AppendReq(AppendRequest<'a>),
    AppendResp(AppendResponse<'a>),
    SealReq(SealRequest<'a>),
    SealResp(SealResponse<'a>),
}
