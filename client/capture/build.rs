fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_FEATURE_RECORDER").is_ok()
    {
        println!("cargo:rustc-link-arg-bins=/DELAYLOAD:obs.dll");
        println!("cargo:rustc-link-arg-bins=delayimp.lib");
    }
}
