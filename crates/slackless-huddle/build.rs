fn main() {
    println!("cargo:rerun-if-changed=proto/SignalingProtocol.proto");
    let fds = protox::compile(["proto/SignalingProtocol.proto"], ["proto"])
        .expect("compile Chime SignalingProtocol.proto with protox");
    prost_build::Config::new()
        .compile_fds(fds)
        .expect("generate prost types for SignalingProtocol.proto");
}
