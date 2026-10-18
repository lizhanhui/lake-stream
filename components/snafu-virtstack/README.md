# SNAFU Virtual Stack Trace

This crate follows GreptimeDB's SNAFU stack-style error formatting as a reusable crate. The port baseline is GreptimeDB commit `00f0dcc663dbb6a933212179b1b54a6129f038f5`:

- `src/common/error/src/ext.rs` — `StackError`
- `src/common/macro/src/stack_trace_debug.rs` — `#[stack_trace_debug]`

The implementation intentionally stays close to GreptimeDB so later upstream changes are easy to sync. The generated trait path is the only required adaptation: GreptimeDB's `::common_error::ext::StackError` becomes `::snafu_virtstack::StackError`.

## Usage

```toml
[dependencies]
snafu = "0.9"
snafu-virtstack = { path = "components/snafu-virtstack" }
```

```rust
use snafu::{Location, ResultExt, Snafu};
use snafu_virtstack::stack_trace_debug;

#[derive(Snafu)]
#[stack_trace_debug]
enum StorageError {
    #[snafu(display("Failed to read configuration"))]
    ReadConfig {
        #[snafu(source)]
        error: std::io::Error,
        #[snafu(implicit)]
        location: Location,
    },
}

#[derive(Snafu)]
#[stack_trace_debug]
enum AppError {
    #[snafu(display("Failed to initialize storage"))]
    InitializeStorage {
        source: StorageError,
        #[snafu(implicit)]
        location: Location,
    },
}

fn read_config() -> Result<String, StorageError> {
    std::fs::read_to_string("config.json").context(ReadConfigSnafu)
}

fn initialize() -> Result<(), AppError> {
    read_config().context(InitializeStorageSnafu)?;
    Ok(())
}
```

Formatting the resulting `AppError` with `Debug` produces an outer-to-inner stack:

```text
0: Failed to initialize storage, at src/main.rs:35:19
1: Failed to read configuration, at src/main.rs:31:49
2: Os { code: 2, kind: NotFound, message: "No such file or directory" }
```

Locations are captured when SNAFU constructs each context. Formatting does not call `Location::caller()` and does not invent a location for an external error.

## Error conventions

The macro follows GreptimeDB's field-name convention:

- `location`: the context's `#[snafu(implicit)]` location.
- `source`: an internal error that also implements `StackError`; formatting recurses into it.
- `error`: an external error with `#[snafu(source)]`; formatting appends its `Debug` representation as the terminal line.

Only internal errors may be named `source`. Standard-library and third-party errors must be named `error`.

A same-type `?` only returns the existing error and does not construct a context, so it does not add a stack layer. Use `.context(...)`, `.with_context(...)`, a SNAFU selector, or a transparent conversion when a new context and location are required.

A variant without `location` is supported, but its line has no `, at file:line:column` suffix.

## Transparent errors

For `#[snafu(transparent)]`, stack-style `Debug` renders the wrapper as `<transparent>` and `StackError::transparent()` returns `true`. Ordinary SNAFU `Display` continues to delegate to the source.

## API

```rust
pub trait StackError: std::error::Error {
    fn debug_fmt(&self, layer: usize, buf: &mut Vec<String>);
    fn next(&self) -> Option<&dyn StackError>;
    fn last(&self) -> &dyn StackError
    where
        Self: Sized;
    fn transparent(&self) -> bool;
}
```

`#[stack_trace_debug]` implements `StackError` and replaces the enum's `Debug` implementation with GreptimeDB's numbered stack format. `StackError` is also implemented for `Arc<T>` and `Box<T>` using the same bounds and forwarding behavior as GreptimeDB.

## Upstream-compatible limitations

The initial port deliberately preserves upstream behavior:

- Every variant needs `#[snafu(display(...))]` or `#[snafu(transparent)]`.
- Special fields are recognized by the exact names `location`, `source`, and `error`.
- The generated path requires the crate to be accessible as `snafu_virtstack`.
- Generic error enums are not supported by the current upstream macro.
- Only a variant-level `#[cfg]` attribute is copied to generated match arms; `cfg_attr` is not.
- SNAFU-compatible named and unit variants are supported.

## License and provenance

The port and GreptimeDB are licensed under Apache License 2.0.
