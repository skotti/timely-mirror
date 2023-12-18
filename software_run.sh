#!/bin/bash
# Provide as parameters in this order: 
# 1) type of benchmark (xdma, eci, pci);
# 2) number of Timely Dataflow rounds
# 3) data size (number of elements, 16, 32, 64, ...);
# 4) number of operators (for now just 1)

# (!) Specify the path where you would like to run the benchmark
run_dir=/home/enzian/
# (!) Specify the path for compiling Timely Dataflow ( the root directory of Timely Dataflow)
timely_dir=/scratch/aruzhans/timely-on-fpga-different-systems/
# (!) Specify the path to shared library
shared_dir=/scratch/aruzhans/timely-dataflow-on-hardware/xdma_shim/

benchmark_name=benchmark_$1
cd "$timely_dir"
cargo build --features $1 --release --example "$benchmark_name"
cp $timely_dir/target/release/examples/benchmark_xdma "$run_dir"

cd "$shared_dir"
make
cp $shared_dir/libxdma_shim.so "$run_dir"

cd "$run_dir"

command="LD_LIBRARY_PATH="$run_dir" ./"$benchmark_name" $2 $3 $4"

sudo bash -c "$command"

