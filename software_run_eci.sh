#!/bin/bash

# Provide as parameters in this order: 
# 1) type of benchmark (xdma, eci, pci);
# 2) number of Timely Dataflow rounds
# 3) data size (number of elements, 16, 32, 64, ...), should be same as in the configuration for the FPGA bitstream;
# 4) number of operators (for now just 1)

# (!) Specify the path where you would like to run the benchmark
# Place the script to the run folder
run_dir=/home/enzian/
# (!) Specify the path for compiling Timely Dataflow ( the root directory of Timely Dataflow)
timely_dir=/scratch/aruzhans/timely-on-fpga-different-systems-eci-testing/timely-on-fpga-different-systems/
# (!) Specify the path to shared library
shared_dir=/scratch/aruzhans/timely-dataflow-on-hardware/xdma_shim/
# (!) Specify the path to the driver dir
driver_dir=/scratch/aruzhans/timely-dataflow-on-hardware/eci_driver/

cd "$driver_dir"
make clean
make
sudo insmod enzian_memory.ko

first_arg="$1"

first_word=$(echo "$first_arg" | cut -d',' -f1)

echo $first_word

benchmark_name=benchmark_$first_word

cd "$timely_dir"
cargo build --features $1 --release --example "$benchmark_name"
cp $timely_dir/target/release/examples/benchmark_eci "$run_dir"

cd "$shared_dir"
make
cp $shared_dir/libxdma_shim.so "$run_dir"
cp $shared_dir/libxdma_shim.so "$timely_dir"/timely/

cd "$run_dir"

command="LD_LIBRARY_PATH="$run_dir" ./"$benchmark_name" $2 $3 $4"

sudo bash -c "$command"

