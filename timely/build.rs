fn main() {
    println!(r"cargo:rustc-link-search=/home/sgtest/timely-on-fpga-without-async_2/timely-on-fpga/timely/");
    println!(r"cargo:rustc-link-lib=fpgalibrary");
}
