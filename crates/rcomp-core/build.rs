fn main() {
    // unrar_sys (used by the `rar` feature) references advapi32 symbols on
    // Windows (registry + CryptAPI calls) but its own build script does not
    // link against advapi32, causing LNK2019 errors on MSVC.  Supplying the
    // link here propagates it to every downstream binary.
    if cfg!(windows) && std::env::var("CARGO_FEATURE_RAR").is_ok() {
        println!("cargo:rustc-link-lib=advapi32");
    }
}
