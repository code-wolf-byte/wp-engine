fn main() {
    // `cef-dll-sys` drops libcef.so (and Chromium's .pak/locales blobs) next to
    // the built binary, but nothing tells the dynamic loader to look there, so
    // the binary dies at startup with "libcef.so: cannot open shared object
    // file" unless the caller sets LD_LIBRARY_PATH. An $ORIGIN rpath makes the
    // executable find its own directory. Only emitted with the `web` feature —
    // default builds have no libcef to find.
    if std::env::var_os("CARGO_FEATURE_WEB").is_some() && cfg!(target_os = "linux") {
        println!("cargo::rustc-link-arg-bins=-Wl,-rpath,$ORIGIN");
    }
    println!("cargo::rerun-if-env-changed=CARGO_FEATURE_WEB");
}
