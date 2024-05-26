fn main() {
    prost_build::compile_protos(
        &["proto/risdb_messages.proto"],
        &["proto/"]
    ).unwrap();
}