use std::fs::{create_dir_all, File};
use std::io::Write;
use std::path::Path;
use rcgen::{CertifiedKey, generate_simple_self_signed};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::compile_protos("proto/raftgrpc.proto")?;
    tonic_build::compile_protos("proto/risdb.proto")?;
    tonic_build::compile_protos("proto/shared.proto")?;

    let output = std::process::Command::new(env!("CARGO"))
        .arg("locate-project")
        .arg("--workspace")
        .arg("--message-format=plain")
        .output()
        .unwrap()
        .stdout;
    let cargo_path = Path::new(std::str::from_utf8(&output).unwrap().trim());
    let workspace_base = cargo_path.parent().unwrap().to_path_buf();

    // TODO: Make it so certs are only regenerated if the directory doesn't exist
    for i in 0..5 {
        let subject_alt_names = vec!["localhost".to_string()];
        let CertifiedKey { cert, key_pair } = generate_simple_self_signed(subject_alt_names).unwrap();

        let base_dir = workspace_base.join(format!("certs/key_{}/", i));
        let cert_path = base_dir.join("server.crt");
        let priv_key_path = base_dir.join("priv.key");
        let pub_key_path = base_dir.join("key.pem");

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
