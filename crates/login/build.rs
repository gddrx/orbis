//! Builds `cpp/login.cpp`, the bit of C++ between the web engine and Rust:
//! it initializes Qt WebEngine before the application object and hands the
//! profile's cookies to Rust. Neither is reachable through Qt Bridge or QML.

fn main() {
    println!("cargo::rerun-if-changed=cpp/login.cpp");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }
    // Link lines are printed after the C++ library's own: with `--as-needed`
    // a shared library listed before its first user is dropped.
    let webengine = pkg_config::Config::new()
        .atleast_version("6.0")
        .cargo_metadata(false)
        .probe("Qt6WebEngineQuick")
        .unwrap_or_else(|e| {
            panic!("Qt WebEngine is needed for the sign-in window (openSUSE: qt6-webenginequick-devel): {e}")
        });
    let mut build = cc::Build::new();
    build.cpp(true).std("c++17").file("cpp/login.cpp");
    for dir in &webengine.include_paths {
        build.include(dir);
    }
    for (name, value) in &webengine.defines {
        build.define(name, value.as_deref());
    }
    build.compile("orbis_login");
    for dir in &webengine.link_paths {
        println!("cargo::rustc-link-search=native={}", dir.display());
    }
    for lib in &webengine.libs {
        println!("cargo::rustc-link-lib={lib}");
    }
}
