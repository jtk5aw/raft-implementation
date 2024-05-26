fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::compile_protos("proto/raftgrpc.proto")?;
    tonic_build::compile_protos("proto/shared.proto")?;

    Ok(())
}
