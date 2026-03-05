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

    // Gzip large PWA assets → OUT_DIR so pwa_assets.rs can include_bytes! them
    let dist = PathBuf::from("../pwa-wasm/dist");
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
