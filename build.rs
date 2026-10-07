use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn collect(root: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, files);
        } else if path.is_file() {
            files.push(path);
        }
    }
}

fn main() {
    println!("cargo:rerun-if-changed=locales");
    let mut catalogs = String::from("pub static NATIVE_CATALOGS: &[(&str, &str)] = &[\n");
    let mut locale_files = Vec::new();
    collect(Path::new("locales/native"), &mut locale_files);
    locale_files.sort();
    for file in locale_files {
        if file.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        catalogs.push_str(&format!(
            "({:?}, include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), {:?}))),\n",
            file.file_stem().unwrap().to_str().unwrap(),
            format!("/{}", file.to_str().unwrap())
        ));
    }
    catalogs.push_str("];\n");
    fs::write(
        PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("language_catalogs.rs"),
        catalogs,
    )
    .unwrap();
    println!("cargo:rerun-if-changed=web/dist");
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    assert!(
        root.join("web/dist/index.html").is_file(),
        "缺少网页构建产物；请先执行 npm --prefix web ci --ignore-scripts --registry=https://registry.npmjs.org，再执行 npm --prefix web run build"
    );
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rerun-if-changed=native/notifications");
        println!("cargo:rerun-if-changed=scripts/build-notification-helper.sh");
        let executable =
            PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("CodePerimeterNotifications");
        let status = Command::new("/bin/sh")
            .arg(root.join("scripts/build-notification-helper.sh"))
            .arg(executable)
            .arg(std::env::var("TARGET").unwrap())
            .status()
            .expect("无法启动原生通知构建；请安装 Xcode Command Line Tools");
        assert!(status.success(), "原生通知助手构建失败");
    }
    let mut files = Vec::new();
    collect(&root.join("web/dist"), &mut files);
    files.sort();
    let mut generated = String::from("pub static ASSETS: &[(&str, &[u8])] = &[\n");
    for file in files {
        let relative = file.strip_prefix(root.join("web/dist")).unwrap();
        generated.push_str(&format!(
            "({:?}, include_bytes!(concat!(env!(\"CARGO_MANIFEST_DIR\"), {:?}))),\n",
            relative.to_str().unwrap(),
            format!("/web/dist/{}", relative.to_str().unwrap())
        ));
    }
    generated.push_str("];\n");
    fs::write(
        PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("web_assets.rs"),
        generated,
    )
    .unwrap();
}
