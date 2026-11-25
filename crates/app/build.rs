//! Builds `cpp/window.cpp`, the bit of C++ that asks the compositor, through
//! KF6 WindowSystem, for the blur behind the window and for focus with an
//! activation token: Qt Bridge gives Rust no handle of the window.

fn main() {
    println!("cargo::rerun-if-changed=cpp/window.cpp");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }
    // Link lines are printed after the C++ library's own: with `--as-needed`
    // a shared library listed before its first user is dropped.
    let kwindowsystem = pkg_config::Config::new()
        .atleast_version("6.0")
        .cargo_metadata(false)
        .probe("KF6WindowSystem")
        .unwrap_or_else(|e| {
            panic!(
                "KF6 WindowSystem is needed for the blur behind the window and its focus \
                 (openSUSE: kf6-kwindowsystem-devel): {e}"
            )
        });
    let mut build = cc::Build::new();
    build.cpp(true).std("c++17").file("cpp/window.cpp");
    for dir in &kwindowsystem.include_paths {
        build.include(dir);
    }
    for (name, value) in &kwindowsystem.defines {
        build.define(name, value.as_deref());
    }
    build.compile("orbis_window");
    for dir in &kwindowsystem.link_paths {
        println!("cargo::rustc-link-search=native={}", dir.display());
    }
    for lib in &kwindowsystem.libs {
        println!("cargo::rustc-link-lib={lib}");
    }
}
