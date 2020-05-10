extern crate bindgen;
extern crate pkg_config;

use std::env;
use std::path::PathBuf;

pub fn main() {
    // link to the fuse lib
    println!("cargo:rustc-link-lib=dylib=fuse");

    // rebuild wrapper when needed
    println!("cargo:rerun-if-changed=wrapper.h");
    pkg_config::Config::new()
        .atleast_version("2.9.6")
        .probe("fuse")
        .unwrap();

    let bindings = bindgen::Builder::default()
        .header("wrapper.h")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks))
        .generate()
        .expect("Could not generate bindings");

    let out = PathBuf::from(env::var("OUT_DIR").unwrap());

    // write to bindings.rs
    bindings
        .write_to_file(out.join("bindings.rs"))
        .expect("Could not write bindings file");
}
