fn main() {
    println!(r"cargo:rustc-link-search=/home/enzian/timely/timely-on-fpga/timely/");
    println!(r"cargo:rustc-link-lib=fpgalibrary");
}
