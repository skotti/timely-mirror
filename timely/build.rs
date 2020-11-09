fn main() {
    println!(r"cargo:rustc-link-search=/home/aruzhans/timely-on-fpga/timely");
    println!(r"cargo:rustc-link-lib=fpgalibrary");
}
