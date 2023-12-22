use std::env;
use std::path::Path;

fn main() {

    // Get the absolute path of the library directory based on the build.rs script location

    //let relative_library_path = "";
    let script_dir = env::var("CARGO_MANIFEST_DIR").unwrap();

    let absolute_library_path = Path::new(&script_dir);

    // Convert the absolute path to a string
    let absolute_library_path_str = absolute_library_path.to_str().unwrap();

    // Set the linker search path using the absolute path
    println!("cargo:rustc-link-search={}", absolute_library_path_str);

    //println!(r"cargo:rustc-link-search=/scratch/aruzhans/timely-on-fpga-different-systems-eci-testing/timely-on-fpga-different-systems/timely");
    println!(r"cargo:rustc-link-lib=xdma_shim");
}

