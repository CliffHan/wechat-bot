use std::{env, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    const PROTOC_PATH: &str = "protoc-27.3-win64";
    const WCF_PATH: &str = "wcf-v39.2.4";
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo::rerun-if-changed={}", WCF_PATH);

    // copy dll to out dir
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let target_dir = out_dir.ancestors().nth(3).unwrap().to_path_buf();
    let dlls = vec!["sdk.dll", "spy.dll", "spy_debug.dll"];
    let dll_dir = PathBuf::from(&manifest_dir).join(WCF_PATH).canonicalize().unwrap();
    for dll in dlls {
        let src_path = dll_dir.join(dll);
        let dest_path = target_dir.join(dll);
        if let Err(e) = fs::copy(&src_path, &dest_path) {
            println!("cargo:warning=failed to copy {:?} to {:?}, error={}", src_path, dest_path, e);
        }
    }

    // configure protobuf tools
    let protobuf_location = PathBuf::from(&manifest_dir).join(PROTOC_PATH).canonicalize().unwrap();
    let protoc = protobuf_location.join("bin/protoc.exe").canonicalize().unwrap();
    let protoc_include = protobuf_location.join("include").canonicalize().unwrap();
    env::set_var("PROTOBUF_LOCATION", protobuf_location.to_str().unwrap());
    env::set_var("PROTOC", protoc.to_str().unwrap());
    env::set_var("PROTOC_INCLUDE", protoc_include.to_str().unwrap());

    // build wcf proto
    let wcf_protos = format!("{}/proto", WCF_PATH);
    let wcf_proto = format!("{}/wcf.proto", &wcf_protos);
    let roomdata_proto = format!("{}/roomdata.proto", &wcf_protos);
    tonic_build::configure()
        .build_client(true)
        .build_server(false)
        .type_attribute("wcf.Functions", "#[allow(clippy::enum_variant_names)]")
        .compile(&[wcf_proto.as_str(), roomdata_proto.as_str()], &[wcf_protos.as_str()])
        .unwrap();

    Ok(())
}
