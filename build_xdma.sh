#!/bin/bash
/scratch/aruzhans/.cargo/bin/cargo build --features xdma,16op --release --example benchmark_xdma
cp target/release/examples/benchmark_xdma ~/
