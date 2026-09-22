fn main() {
    // obs.dll non e' accanto all'eseguibile in fase di compilazione, solo dopo che
    // l'installer (install.rs) l'ha scaricata: il collegamento e' quindi ritardato al
    // primo utilizzo vero, non alla partenza del programma.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_FEATURE_RECORDER").is_ok()
    {
        println!("cargo:rustc-link-arg-bins=/DELAYLOAD:obs.dll");
        println!("cargo:rustc-link-arg-bins=delayimp.lib");
    }
}
