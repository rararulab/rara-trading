use protox::prost::Message;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let file_descriptors = protox::compile(["proto/rara.proto"], ["proto/"])?;
    let file_descriptor_bytes = file_descriptors.encode_to_vec();

    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR")?);
    let descriptor_path = out_dir.join("rara_descriptor.bin");
    std::fs::write(&descriptor_path, &file_descriptor_bytes)?;

    tonic_build::configure()
        .skip_protoc_run()
        .file_descriptor_set_path(&descriptor_path)
        .compile_protos(&["proto/rara.proto"], &["proto/"])?;

    Ok(())
}
