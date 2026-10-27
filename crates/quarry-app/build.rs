// Embeds the app icon in quarry.exe so Explorer, the taskbar and installer shortcuts show it
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("../../assets/icon/quarry.ico")
            .compile()
            .expect("embed Windows icon");
    }
}
