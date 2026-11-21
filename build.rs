fn main() {
    // The icon resource is what makes Explorer, the taskbar and Settings use
    // our icon, not just the window at runtime.
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    embed_resource::compile("assets/app.rc", embed_resource::NONE)
        .manifest_optional()
        .expect("compile assets/app.rc");
}
