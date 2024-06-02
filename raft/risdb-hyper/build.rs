// TODO: Long term it could be cool to look into creating a ServiceGenerator that hooks into this proto file
// A lot of the code I'm writing could be written as a code generator. I know at the end of the day that's what
// tonic serves to be but I don't need all the mess of a gRPC and protobuf is still useful. Could be worth looking into

fn main() {
    prost_build::compile_protos(&["proto/risdb_messages.proto"], &["proto/"]).unwrap();
}
