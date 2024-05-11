#!/bin/bash
#/scratch/aruzhans/.cargo/bin/cargo build --release --example benchmark_cpu_32
/scratch/aruzhans/.cargo/bin/cargo build --features xdma,16op --release --example benchmark_cpu_32


