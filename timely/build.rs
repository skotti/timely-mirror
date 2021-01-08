fn main() {
    println!(r"cargo:rustc-link-search=/home/skotti/timely_code_clion/timely-dataflow/timely");
    println!(r"cargo:rustc-link-lib=fpgalibrary");
}
