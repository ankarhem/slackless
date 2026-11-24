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

    #[cfg(target_os = "macos")]
    {
        println!("cargo:rerun-if-changed=native/macos_media.m");
        cc::Build::new()
            .file("native/macos_media.m")
            .flag("-fobjc-arc")
            .flag("-Wno-deprecated-declarations")
            .compile("slackless_macos_media");
        for fw in [
            "AVFoundation",
            "Accelerate",
            "AudioToolbox",
            "CoreAudio",
            "CoreGraphics",
            "CoreImage",
            "CoreMedia",
            "CoreVideo",
            "Foundation",
            "Metal",
            "ScreenCaptureKit",
            "VideoToolbox",
            "Vision",
        ] {
            println!("cargo:rustc-link-lib=framework={fw}");
        }
    }
}
