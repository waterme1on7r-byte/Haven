fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        if let Err(error) = winres::WindowsResource::new()
            .set_icon("assets/haven.ico")
            .compile()
        {
            panic!("failed to embed Haven's Windows icon: {error}");
        }
    }
}
