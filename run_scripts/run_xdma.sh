#!/bin/bash

# rounds data operators

# nightly build of rust should be installed on the running machine to activate pcie vector instructions
# running path this is the path you are running the script in , the home directory of enzian for example
# $1 is number of iterations, $2 is the number of data items

path=/scratch/aruzhans/timely_all_three
running_path=/home/enzian/test_runs
xdma_driver_folder=$path/xdma_driver/xdma
xdma_shim=$path/testing_with_shim
cd $xdma_shim
make clean
make
cp libxdma_shim.so $running_path

cd $xdma_driver_folder
make clean
make
sudo insmod $xdma_driver_folder/xdma.ko

cd "$path"
./build_xdma.sh

cd "$running_path"


cp $path/target/release/examples/benchmark_xdma $running_path

sudo LD_LIBRARY_PATH=$running_path ./benchmark_xdma $1 $2 31
