extern crate timely;
extern crate hdrhist;

use timely::dataflow::{InputHandle, ProbeHandle};
use timely::dataflow::operators::{Input, Exchange, Inspect, Probe, FpgaWrapperXDMA};
use std::time::{Duration, Instant};


fn main() {

    timely::execute_from_args(std::env::args(), std::env::args(),  |worker, hc, params| {
        //let index = worker.index();
        let mut input = InputHandle::new();
        let mut probe = ProbeHandle::new();

        // create a new input, exchange data, and inspect its output
        let num_rounds = params.rounds;
        let num_data = params.data;
        let num_operators = params.operators;

        worker.dataflow(|scope| {
            scope.input_from(&mut input)
                 .fpga_wrapper_xdma(num_data, num_operators, hc)
                 //.inspect(move |x| println!("worker {}:\thello {}", index, x))
                 .probe_with(&mut probe);
        });
	
        // introduce data and watch!
        let start = Instant::now();
        let mut epoch_start = Instant::now();
        let mut hist = hdrhist::HDRHist::new();
        
        for round in 0..num_rounds {
	        for _j in 0..num_data {
                input.send(round as u64 + 21);// max = 0
	        }
            
            input.advance_to(round as u64 + 1);

            while probe.less_than(input.time()) {
                worker.step();
            }
            let epoch_end = Instant::now();
            let epoch_nanos = (epoch_end - epoch_start).as_nanos();
            epoch_start = epoch_end;
            hist.add_value(epoch_nanos as u64);
        }

        //let epoch_end = Instant::now();

        let total_nanos = (Instant::now() - start).as_nanos();
        let epoch_latency = (total_nanos as f64) / 1_000_000_000f64 / (num_rounds as f64); // sec
        let epoch_throughput = (num_rounds as f64) / (total_nanos as f64) * 1_000_000_000f64; // epochs/sec
	    println!("epoch time: {}", epoch_latency); 

        println!("total time (nanos): {}, throughput: {}", total_nanos, epoch_throughput);
        println!("epoch latency (nanos):\n{}", hist.summary_string());

    }).unwrap();
	dbg!("Done");
}