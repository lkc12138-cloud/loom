//! Native macOS desktop wrapper (`Loom.app`). Compiles embedded Swift source with system `swiftc`.
use crate::config::{self, Result};
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::{
    fs,
    process::{Command, Stdio},
};

#[cfg(target_os = "macos")]
const INFO_PLIST: &str = include_str!("../app/macos/Info.plist");
#[cfg(target_os = "macos")]
const SWIFT_SOURCE: &str = include_str!("../app/macos/LoomApp.swift");

#[cfg(target_os = "macos")]
const ICON_GEN_SWIFT: &str = r#"import Cocoa

let args = CommandLine.arguments
guard args.count == 2 else { exit(1) }
let outDir = URL(fileURLWithPath: args[1])

let sizes: [(String, Int)] = [
    ("icon_16x16.png", 16),
    ("icon_16x16@2x.png", 32),
    ("icon_32x32.png", 32),
    ("icon_32x32@2x.png", 64),
    ("icon_128x128.png", 128),
    ("icon_128x128@2x.png", 256),
    ("icon_256x256.png", 256),
    ("icon_256x256@2x.png", 512),
    ("icon_512x512.png", 512),
    ("icon_512x512@2x.png", 1024),
]

for (name, px) in sizes {
    let size = NSSize(width: px, height: px)
    let rep = NSBitmapImageRep(
        bitmapDataPlanes: nil,
        pixelsWide: px,
        pixelsHigh: px,
        bitsPerSample: 8,
        samplesPerPixel: 4,
        hasAlpha: true,
        isPlanar: false,
        colorSpaceName: .deviceRGB,
        bytesPerRow: 0,
        bitsPerPixel: 0
    )!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)

    let rect = NSRect(origin: .zero, size: size)
    let inset = CGFloat(px) * 0.06
    let bgRect = rect.insetBy(dx: inset, dy: inset)
    let radius = CGFloat(px) * 0.22
    let path = NSBezierPath(roundedRect: bgRect, xRadius: radius, yRadius: radius)
    NSColor(calibratedRed: 0.082, green: 0.341, blue: 0.259, alpha: 1.0).setFill()
    path.fill()

    let fontSize = CGFloat(px) * 0.56
    let font = NSFont.systemFont(ofSize: fontSize, weight: .bold)
    let attrs: [NSAttributedString.Key: Any] = [
        .font: font,
        .foregroundColor: NSColor.white,
    ]
    let glyph = NSAttributedString(string: "⌘", attributes: attrs)
    let gSize = glyph.size()
    let point = NSPoint(
        x: (CGFloat(px) - gSize.width) / 2.0,
        y: (CGFloat(px) - gSize.height) / 2.0 - CGFloat(px) * 0.02
    )
    glyph.draw(at: point)

    NSGraphicsContext.restoreGraphicsState()
    if let png = rep.representation(using: .png, properties: [:]) {
        try png.write(to: outDir.appendingPathComponent(name))
    }
}
"#;

pub fn default_app_dir() -> PathBuf {
    config::home().join("Applications")
}

#[cfg(target_os = "macos")]
struct TempDir(PathBuf);

#[cfg(target_os = "macos")]
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(target_os = "macos")]
fn source_digest() -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(INFO_PLIST.as_bytes());
    h.update(SWIFT_SOURCE.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(target_os = "macos")]
fn generate_icns(work: &Path, dest_icns: &Path) -> bool {
    if !Path::new("/usr/bin/iconutil").is_file() {
        return false;
    }
    let iconset = work.join("AppIcon.iconset");
    if fs::create_dir_all(&iconset).is_err() {
        return false;
    }
    let script = work.join("gen_icon.swift");
    if fs::write(&script, ICON_GEN_SWIFT).is_err() {
        return false;
    }
    let ok = Command::new("/usr/bin/swift")
        .arg(&script)
        .arg(&iconset)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        return false;
    }
    Command::new("/usr/bin/iconutil")
        .args(["-c", "icns"])
        .arg(&iconset)
        .arg("-o")
        .arg(dest_icns)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(target_os = "macos")]
pub fn build_app(app_dir: Option<&Path>, rebuild: bool) -> Result<PathBuf> {
    let target_dir = app_dir.map(PathBuf::from).unwrap_or_else(default_app_dir);
    fs::create_dir_all(&target_dir)?;

    let bundle = target_dir.join("Loom.app");
    let contents = bundle.join("Contents");
    let macos_dir = contents.join("MacOS");
    let resources_dir = contents.join("Resources");
    let binary = macos_dir.join("Loom");
    let digest_path = resources_dir.join("source.sha256");
    let digest = source_digest();

    if !rebuild
        && binary.is_file()
        && contents.join("Info.plist").is_file()
        && fs::read_to_string(&digest_path).ok().as_deref() == Some(digest.as_str())
    {
        return Ok(bundle);
    }

    if !Path::new("/usr/bin/swiftc").is_file() {
        return Err("需要 macOS Xcode Command Line Tools（/usr/bin/swiftc）来构建 Loom.app。请先运行 xcode-select --install。".into());
    }

    let work = TempDir(target_dir.join(format!(".loom-app-build-{:016x}", rand::random::<u64>())));
    fs::create_dir_all(&work.0)?;
    let swift_file = work.0.join("LoomApp.swift");
    fs::write(&swift_file, SWIFT_SOURCE)?;

    let staged = work.0.join("Loom.app");
    let staged_macos = staged.join("Contents/MacOS");
    let staged_resources = staged.join("Contents/Resources");
    fs::create_dir_all(&staged_macos)?;
    fs::create_dir_all(&staged_resources)?;
    fs::write(staged.join("Contents/Info.plist"), INFO_PLIST)?;
    fs::write(staged_resources.join("source.sha256"), &digest)?;

    let staged_bin = staged_macos.join("Loom");
    let out = Command::new("/usr/bin/swiftc")
        .args([
            "-O",
            "-swift-version",
            "5",
            "-framework",
            "Cocoa",
            "-framework",
            "WebKit",
        ])
        .arg(&swift_file)
        .arg("-o")
        .arg(&staged_bin)
        .stdin(Stdio::null())
        .output()?;
    if !out.status.success() {
        return Err("编译 Loom.app 失败；请确认 Xcode Command Line Tools 已完整安装。".into());
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&staged_bin, fs::Permissions::from_mode(0o755))?;
    }

    let _ = generate_icns(&work.0, &staged_resources.join("AppIcon.icns"));

    if Path::new("/usr/bin/codesign").is_file() {
        let _ = Command::new("/usr/bin/codesign")
            .args(["--force", "--deep", "--sign", "-"])
            .arg(&staged)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    if bundle.exists() {
        let old = work.0.join("Loom.app.old");
        if fs::rename(&bundle, &old).is_err() {
            fs::remove_dir_all(&bundle)?;
        }
    }
    fs::rename(&staged, &bundle)?;
    Ok(bundle)
}

#[cfg(not(target_os = "macos"))]
pub fn build_app(_app_dir: Option<&Path>, _rebuild: bool) -> Result<PathBuf> {
    Err(
        "Loom.app 原生桌面应用目前仅支持 macOS；当前系统请使用 `loom console` 打开 Web 工作台。"
            .into(),
    )
}

pub fn launch_app(
    data: &Path,
    app_dir: Option<&Path>,
    build_only: bool,
    rebuild: bool,
) -> Result<()> {
    let bundle = build_app(app_dir, rebuild)?;
    println!("Loom 桌面应用已就绪：{}", bundle.display());
    if build_only {
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        let mut cmd = std::process::Command::new("/usr/bin/open");
        cmd.arg("-a").arg(&bundle);
        if data != config::data_dir() {
            cmd.arg("--env")
                .arg(format!("MODEL_HUB_DATA_DIR={}", data.display()));
        }
        if !cmd.status()?.success() {
            return Err("无法启动 Loom.app".into());
        }
        println!("已打开 Loom.app，菜单栏图标 ⌘ 可随时开关网关与工作台。");
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = data;
    }
    Ok(())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn builds_and_signs_macos_app_bundle_in_tempdir() {
        let t = tempfile::tempdir().unwrap();
        let bundle = build_app(Some(t.path()), true).unwrap();
        assert!(bundle.join("Contents/MacOS/Loom").is_file());
        assert!(bundle.join("Contents/Info.plist").is_file());
        assert!(bundle.join("Contents/Resources/source.sha256").is_file());
        if Path::new("/usr/bin/codesign").is_file() {
            let status = Command::new("/usr/bin/codesign")
                .args(["--verify", "--deep"])
                .arg(&bundle)
                .status()
                .unwrap();
            assert!(status.success(), "codesign verification failed");
        }
        // Re-calling without rebuild uses cached bundle cleanly.
        let cached = build_app(Some(t.path()), false).unwrap();
        assert_eq!(cached, bundle);
    }
}
