fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let config = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into());
        slint_build::compile_with_config("ui/mimo.slint", config)
            .expect("failed to compile the MIMO interface");
    }
}
