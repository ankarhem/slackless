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
            "AppKit",
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
        // Objective-C `@available(macOS 14.0, *)` emits calls to `___isPlatformVersionAtLeast`
        // when `MACOSX_DEPLOYMENT_TARGET < 14.0`. Because `rustc` passes `-nodefaultlibs`, link
        // Apple's `libclang_rt.osx.a` explicitly when present so release builds with deployment
        // target 13.0 link cleanly on Xcode toolchains.
        let cc_bin = std::env::var("CC").unwrap_or_else(|_| "clang".to_owned());
        if let Ok(out) = std::process::Command::new(&cc_bin)
            .arg("--print-resource-dir")
            .output()
            && out.status.success()
        {
            let res_dir = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            let darwin_lib_dir = std::path::Path::new(&res_dir).join("lib/darwin");
            if darwin_lib_dir.join("libclang_rt.osx.a").is_file() {
                println!(
                    "cargo:rustc-link-search=native={}",
                    darwin_lib_dir.display()
                );
                println!("cargo:rustc-link-lib=static=clang_rt.osx");
            }
        }
    }
}
