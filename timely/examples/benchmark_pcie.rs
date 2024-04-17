extern crate hdrhist;
extern crate timely;

use std::time::Instant;
use timely::dataflow::operators::{FpgaWrapperPCI, Input, Probe};
use timely::dataflow::{InputHandle, ProbeHandle};

fn main() {
    // initializes and runs a timely dataflow.
    timely::execute_from_args(std::env::args(),std::env::args(), |worker, hc, params| {
        let mut input = InputHandle::new();
        let mut probe = ProbeHandle::new();

        // create a new input, exchange data, and inspect its output
        let num_rounds = params.rounds;
        let num_data = params.data;
        let num_operators = params.operators;

        // create a new input, exchange data, and inspect its output
        worker.dataflow(|scope| {
            scope
                .input_from(&mut input)
                .fpga_wrapper_eci(num_data, num_operators, hc)
                .probe_with(&mut probe);
        });

        // introduce data and watch!
        let start = Instant::now();
        let mut epoch_start = Instant::now();
        let mut hist = hdrhist::HDRHist::new();

        let mut epoch_latencies = vec![0; num_rounds as usize];
        for round in 0..num_rounds {
            for _j in 0..num_data {
                input.send(round as u64 + 21); // max = 0
            }
            input.advance_to(round as u64 + 1);
            while probe.less_than(input.time()) {
                worker.step();
            }
            let epoch_end = Instant::now();
            let epoch_nanos = (epoch_end - epoch_start).as_nanos();
            epoch_start = epoch_end;
            hist.add_value(epoch_nanos as u64);
            epoch_latencies[round as usize] = epoch_nanos as u64;
        }

        let epoch_end = Instant::now();
        let total_nanos = (epoch_end - start).as_nanos();
        let epoch_latency = (total_nanos as f64) / 1_000_000_000f64 / (num_rounds as f64); // sec
        let epoch_throughput = (num_rounds as f64) / (total_nanos as f64) * 1_000_000_000f64; // epochs/sec
        println!("Rounds: {num_rounds}");
        println!("Batch size: {num_data}");
        // println!("epoch time (sec): {}", epoch_latency);
        // println!("total time (nanos): {}", total_nanos);
        // println!("epoch throughput (epochs/sec): {}", epoch_throughput);
        println!("epoch latencies (nanos):");
        for elem in epoch_latencies {
            println!("{elem}");
        }
    })
    .unwrap();
}
