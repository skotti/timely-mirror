#!/bin/bash
/scratch/aruzhans/.cargo/bin/cargo build --features xdma,32op --release --example benchmark_xdma
cp target/release/examples/benchmark_xdma ~/
