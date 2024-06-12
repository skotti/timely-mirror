#!/bin/bash
# nightly build of rust should be installed on the running machine to activate pcie vector instructions
# $1 is number of iterations, $2 is the number of data items

path=/scratch/aruzhans/timely_all_three
running_path=/home/enzian/test_runs
eci_driver_folder=$path/eci_driver

cd $eci_driver_folder
make clean
make
sudo insmod $eci_driver_folder/enzian_memory.ko


cd "$path"
./build_eci_32.sh

cd "$running_path"

# rounds data operators

cp $path/target/release/examples/benchmark_eci $running_path
cp $path/libxdma.so $running_path
sudo LD_LIBRARY_PATH=$running_path ./benchmark_eci $1 $2 31
