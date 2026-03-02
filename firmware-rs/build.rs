fn main() {
    // Tell cargo to re-run if memory.x changes
    println!("cargo:rerun-if-changed=memory.x");

    // Put memory.x on the linker search path
    println!("cargo:rustc-link-search=.");
}
