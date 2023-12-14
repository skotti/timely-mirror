fn main() {
    println!(r"cargo:rustc-link-search=/scratch/aruzhans/timely-on-fpga-different-systems/timely");
    println!(r"cargo:rustc-link-lib=xdma_shim");
}

