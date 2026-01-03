//! Build script for Sluice.
//!
//! Compiles proto/sluice/v1/sluice.proto into Rust code using tonic-build.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Compile the proto file
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&["proto/sluice/v1/sluice.proto"][..], &["proto"][..])?;

    // Re-run if proto file changes
    println!("cargo:rerun-if-changed=proto/sluice/v1/sluice.proto");

    Ok(())
}
