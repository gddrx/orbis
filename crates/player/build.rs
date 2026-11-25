// Links libmpv (the `mpv-devel` package provides libmpv.so and its headers).
fn main() {
    println!("cargo::rustc-link-lib=mpv");
}
