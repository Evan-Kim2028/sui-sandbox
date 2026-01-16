fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Only compile proto if the proto directory exists
    let proto_dir = std::path::Path::new("proto");
    if !proto_dir.exists() {
        return Ok(());
    }

    // Create output directory if it doesn't exist
    std::fs::create_dir_all("src/grpc/generated")?;

    // Configure prost to use proto3 optional (requires newer protoc or bundled)
    let mut config = prost_build::Config::new();
    config.protoc_arg("--experimental_allow_proto3_optional");

    // Map google.rpc to our local module path
    config.extern_path(".google.rpc", "crate::grpc::google::rpc");

    // Configure tonic to compile Sui gRPC protos
    tonic_build::configure()
        .build_server(false) // We only need client
        .build_client(true)
        .out_dir("src/grpc/generated")
        .compile_protos_with_config(
            config,
            &[
                "proto/sui/rpc/v2/ledger_service.proto",
                "proto/sui/rpc/v2/subscription_service.proto",
            ],
            &["proto"],
        )?;

    println!("cargo:rerun-if-changed=proto/");

    Ok(())
}
