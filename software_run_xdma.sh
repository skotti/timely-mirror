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
# (!) Specify the path to the driver
driver_dir=/home/enzian/linux-kernel/xdma/

#cd "$driver_dir"
#make clean
#make
#sudo insmod xdma.ko

benchmark_name=benchmark_xdma

cd "$timely_dir"
cargo build --features $1 --example "$benchmark_name"
cp $timely_dir/target/debug/examples/benchmark_xdma "$run_dir"

#cd "$shared_dir"
#make
#cp $shared_dir/libxdma_shim.so "$run_dir"
#cp $shared_dir/libxdma_shim.so "$timely_dir"/timely/

cd "$run_dir"

command="LD_LIBRARY_PATH="$run_dir" ./"$benchmark_name" $2 $3 $4"

sudo bash -c "$command"

