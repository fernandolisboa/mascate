//! Embeds the app icon in the Windows executable (#69). GPUI loads icon
//! resource 1 for the window, the taskbar and Alt+Tab; elsewhere this does nothing.

fn main() {
    println!("cargo:rerun-if-changed=mascate.rc");
    println!("cargo:rerun-if-changed=assets/mascate.ico");
    embed_resource::compile("mascate.rc", embed_resource::NONE)
        .manifest_optional()
        .expect("compiles the icon resource");
}
