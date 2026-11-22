//! macOS reads an unbundled executable's Info.plist from its `__TEXT,__info_plist` section.
//! Embedding one names the app "Slackless" in the menu bar and the About panel, with the
//! crate's version. It declares no bundle identifier, so notifications still take the
//! unbundled path (`slackless-platform`'s `Notifier`).

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let version = env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION");
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleName</key>
	<string>Slackless</string>
	<key>CFBundleShortVersionString</key>
	<string>{version}</string>
	<key>CFBundleURLTypes</key>
	<array>
		<dict>
			<key>CFBundleURLName</key>
			<string>Slack Deep Link</string>
			<key>CFBundleURLSchemes</key>
			<array>
				<string>slack</string>
			</array>
		</dict>
	</array>
</dict>
</plist>
"#
    );
    let path = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("Info.plist");
    fs::write(&path, plist).expect("write the embedded Info.plist");
    println!(
        "cargo:rustc-link-arg-bins=-Wl,-sectcreate,__TEXT,__info_plist,{}",
        path.display()
    );
}
