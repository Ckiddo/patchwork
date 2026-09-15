fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/patchwork/v1/protocol.proto");
    let mut config = prost_build::Config::new();
    config.protoc_executable(protoc_bin_vendored::protoc_bin_path()?);
    // Authentication/password messages must not acquire an accidental Debug logger.
    config.skip_debug(["."]);
    config.enum_attribute(".patchwork.v1.ErrorCode", "#[derive(Debug)]");
    config.file_descriptor_set_path(
        std::path::PathBuf::from(std::env::var("OUT_DIR")?).join("protocol.bin"),
    );
    config.compile_protos(&["proto/patchwork/v1/protocol.proto"], &["proto"])?;
    Ok(())
}
