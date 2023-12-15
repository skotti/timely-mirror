#!/bin/bash
cargo build --features xdma --release --example benchmark_xdma
cp target/release/examples/benchmark_xdma ~/
