#!/bin/bash

gcc -o output_executable test_31_op.c -L/home/enzian/testing_with_shim -lxdma_shim -Wall -Werror
sudo LD_LIBRARY_PATH=$LD_LIBRARY_PATH:/home/enzian/testing_with_shim/ ./output_executable
