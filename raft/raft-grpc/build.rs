use std::fs::{create_dir_all, File};
use std::io::Write;
use rcgen::{CertifiedKey, generate_simple_self_signed};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::compile_protos("proto/raftgrpc.proto")?;
    tonic_build::compile_protos("proto/risdb.proto")?;
    tonic_build::compile_protos("proto/shared.proto")?;

    // TODO: Make it so certs are only regenerated if the directory doesn't exist
    for i in 0..5 {
        let subject_alt_names = vec!["localhost".to_string()];
        let CertifiedKey { cert, key_pair } = generate_simple_self_signed(subject_alt_names).unwrap();
        
        let base_dir = format!("{}/certs/key_{}/", env!("CARGO_MANIFEST_DIR"), i);
        println!("{}", base_dir);
        let cert_path = format!("{}/server.crt", base_dir);
        let priv_key_path = format!("{}/priv.key", base_dir);
        let pub_key_path = format!("{}/key.pem", base_dir);

        create_dir_all(base_dir)?;
        let mut cert_file = File::create(&cert_path)?;
        cert_file.write_all(cert.pem().as_bytes())?;
        let mut priv_key = File::create(&priv_key_path)?;
        priv_key.write_all(key_pair.serialize_pem().as_bytes())?;
        let mut pub_key = File::create(&pub_key_path)?;
        pub_key.write_all(key_pair.public_key_pem().as_bytes())?;
    }

    Ok(())
}
