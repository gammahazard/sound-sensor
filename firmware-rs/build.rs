use std::env;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let out = &PathBuf::from(env::var_os("OUT_DIR").unwrap());

    let memory_x = include_bytes!("memory.x");
    let mut f = File::create(out.join("memory.x")).unwrap();
    f.write_all(memory_x).unwrap();

    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory.x");

    // Strip <link rel="preload"> / <link rel="modulepreload"> tags from index.html.
    // Trunk generates these but the firmware HTTP server can't serve matching integrity hashes.
    let dist = PathBuf::from("../pwa-wasm/dist");
    {
        let html_src = dist.join("index.html");
        let html = std::fs::read_to_string(&html_src)
            .unwrap_or_else(|e| panic!("cannot read {}: {}", html_src.display(), e));
        let cleaned = strip_preload_links(&html);
        let html_dst = out.join("index.html");
        std::fs::write(&html_dst, cleaned.as_bytes())
            .unwrap_or_else(|e| panic!("cannot write {}: {}", html_dst.display(), e));
        println!("cargo:rerun-if-changed={}", html_src.display());
    }

    // Gzip large PWA assets → OUT_DIR so pwa_assets.rs can include_bytes! them
    for name in &["guardian-pwa.js", "guardian-pwa_bg.wasm"] {
        let src = dist.join(name);
        let dst = out.join(format!("{}.gz", name));
        let gz_file = File::create(&dst)
            .unwrap_or_else(|e| panic!("cannot create {}: {}", dst.display(), e));
        let status = Command::new("gzip")
            .args(["-c", "-9"])
            .arg(&src)
            .stdout(gz_file)
            .status()
            .expect("gzip command failed — is gzip installed?");
        assert!(status.success(), "gzip failed for {}", name);
        println!("cargo:rerun-if-changed={}", src.display());
    }

    println!("cargo:rustc-link-arg-bins=--nmagic");
    println!("cargo:rustc-link-arg-bins=-Tlink.x");
    println!("cargo:rustc-link-arg-bins=-Tdefmt.x");
}

/// Remove `<link ... rel="preload" ...>` and `<link ... rel="modulepreload" ...>` tags.
fn strip_preload_links(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find("<link") {
        let after_tag = &rest[start..];
        if let Some(end) = after_tag.find('>') {
            let tag = &after_tag[..=end];
            if tag.contains("rel=\"preload\"") || tag.contains("rel=\"modulepreload\"") {
                result.push_str(&rest[..start]);
                rest = &rest[start + end + 1..];
                continue;
            }
        }
        // Not a preload link, keep it
        result.push_str(&rest[..start + 5]);
        rest = &rest[start + 5..];
    }
    result.push_str(rest);
    result
}
