#!/bin/bash

# rounds data operators
# nightly build of rust should be installed on the running machine to activate pcie vector instructions
# $1 is number of iterations, $2 is the number of data items
# 


path=/scratch/aruzhans/timely_all_three
running_path=/home/enzian/test_runs
pcie_file=$path/source_files/pcie_$2.rs

cd "$path"
cp $pcie_file $path/timely/src/dataflow/operators/fpga_wrapper_pci.rs
./build_pcie.sh

cd "$running_path"

cp $path/target/release/examples/benchmark_pcie $running_path
cp $path/libxdma_shim.so $running_path

sudo LD_LIBRARY_PATH=$running_path ./benchmark_pcie $1 $2 31
