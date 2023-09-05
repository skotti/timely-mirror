#!/bin/bash

# loop count
N=1000

# Create the 'benchmarks' folder if it doesn't exist
mkdir --parents benchmarks/batch_16/

# Compile
cargo build --release --example benchmark_16

# Loop $N times
for ((i=1; i<=$N; i++))
do
    output_file="benchmarks/batch_16/output_$i.txt"  # Create the output file name with index and folder
    sudo target/release/examples/benchmark_16 > $output_file  # Run the binary and redirect output to the file
    echo "Run $i completed. Output stored in $output_file"
done

echo "All runs completed."
