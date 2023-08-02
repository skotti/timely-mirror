fn main() {
    println!(r"cargo:rustc-link-search=/home/nstern/2022-bsc-nstern-report/syncN/timely-on-fpga/timely/");
    println!(r"cargo:rustc-link-lib=fpgalibrary");
}
