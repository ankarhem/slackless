fn main() {
    #[cfg(target_os = "macos")]
    {
        const OVERLAY_SRC: &str = "native/macos/appkit_overlay.m";
        println!("cargo:rerun-if-changed={OVERLAY_SRC}");
        cc::Build::new()
            .file(OVERLAY_SRC)
            .flag("-fobjc-arc")
            .flag("-Wno-deprecated-declarations")
            .compile("slackless_platform_overlay");
        for fw in ["AppKit", "CoreGraphics", "QuartzCore", "Foundation"] {
            println!("cargo:rustc-link-lib=framework={fw}");
        }
    }
}
