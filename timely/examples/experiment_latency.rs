extern crate timely;
extern crate hdrhist;

use rand::Rng;
use std::time::Instant;
use timely::dataflow::{InputHandle, ProbeHandle};
use timely::dataflow::operators::{Input, Probe, FpgaWrapper};

fn main() {
    let t = Instant::now();
    timely::execute_from_args(std::env::args(), |worker, hc| {
        let _index = worker.index();
        let mut input = InputHandle::new();
        let mut probe = ProbeHandle::new();

        // create a new input, exchange data, and inspect its output
        worker.dataflow(|scope| {
            scope.input_from(&mut input)
                 .fpga_wrapper(hc)
                 .probe_with(&mut probe);
        });

        // introduce data and watch!
        let start = Instant::now();
        let mut epoch_start = Instant::now();
        let mut hist = hdrhist::HDRHist::new();

        let mut rng = rand::XorShiftRng::new_unseeded();

        let num_rounds = 10000;
        for r in 0..num_rounds {
            for _j in 0..64 {
                let value = rng.next_u64();
                input.send(value.into()); // max = 0
            }
            input.advance_to(r + 1);
            worker.step();

            let epoch_end = Instant::now();
            let epoch_nanos = (epoch_end - epoch_start).as_nanos();
            epoch_start = epoch_end;
            hist.add_value(epoch_nanos as u64);
        }

        while probe.less_than(input.time()) {
            worker.step();
        }

        let epoch_end = Instant::now();

        let total_nanos = (epoch_end - start).as_nanos();
        let epoch_latency = (total_nanos as f64) / 1_000_000_000f64 / (num_rounds as f64); // sec
        let epoch_throughput = (num_rounds as f64) / (total_nanos as f64) * 1_000_000_000f64; // epochs/sec
        println!("epoch time: {}", epoch_latency);

        println!(
            "total time (nanos): {}, throughput: {}",
            total_nanos, epoch_throughput
        );
        println!("epoch latency (nanos):\n{}", hist.summary_string());
    }).unwrap();

    println!(
        "total time main (nanos): {}",
        (Instant::now() - t).as_nanos()
    );
}
