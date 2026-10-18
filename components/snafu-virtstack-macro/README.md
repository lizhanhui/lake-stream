# SNAFU Virtual Stack Trace Macro

This is the procedural-macro implementation used by `snafu-virtstack`. Applications should depend on and import from the facade crate rather than using this crate directly:

```rust
use snafu_virtstack::{StackError, stack_trace_debug};
```

The macro references GreptimeDB's `src/common/macro/src/stack_trace_debug.rs` at commit `00f0dcc663dbb6a933212179b1b54a6129f038f5`. The only required implementation adaptation is replacing generated references to `::common_error::ext::StackError` with `::snafu_virtstack::StackError`.

It follows the following conventions:

- `location` is the SNAFU context location.
- `source` is an internal `StackError` and is traversed recursively.
- `#[snafu(source)] error` is an external terminal cause rendered with `Debug`.
- `#[snafu(transparent)]` renders the context as `<transparent>`.

See [`../snafu-virtstack/README.md`](../snafu-virtstack/README.md) for usage, API details, behavior, and upstream-compatible limitations.
