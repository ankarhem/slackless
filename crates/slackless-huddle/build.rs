fn main() {
    println!("cargo:rerun-if-changed=proto/SignalingProtocol.proto");
    println!("cargo:rerun-if-changed=proto/PeerMessage.proto");
    let fds = protox::compile(
        ["proto/SignalingProtocol.proto", "proto/PeerMessage.proto"],
        ["proto"],
    )
    .expect("compile Chime & Slack PeerMessage protos with protox");
    prost_build::Config::new()
        .compile_fds(fds)
        .expect("generate prost types for SignalingProtocol.proto and PeerMessage.proto");
}
