use std::{env, path::PathBuf, process::Command};

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let schema = manifest_dir.join("flatbuffers/proto.fbs");
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("flatbuffers");
    let flatc = env::var_os("FLATC").unwrap_or_else(|| "flatc".into());

    println!("cargo:rerun-if-changed={}", schema.display());
    println!("cargo:rerun-if-env-changed=FLATC");
    println!("cargo:rerun-if-env-changed=PATH");

    let output = Command::new(&flatc)
        .arg("-o")
        .arg(&out_dir)
        .arg("--rust")
        .arg("--rust-module-root-file")
        .arg(&schema)
        .output()
        .unwrap_or_else(|error| {
            panic!(
                "failed to run {flatc:?}: {error}. Install flatc or set FLATC to its executable path"
            )
        });

    if !output.status.success() {
        panic!(
            "{flatc:?} failed to compile {} ({}):\n{}{}",
            schema.display(),
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}
