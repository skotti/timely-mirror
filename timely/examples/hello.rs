extern crate timely;
extern crate hdrhist;

use timely::dataflow::{InputHandle, ProbeHandle};
use timely::dataflow::operators::{Input, Exchange, Inspect, Probe, FpgaWrapper};
//use timely::dataflow::operators::fpga_wrapper::HardwareCommon;
use std::time::{Duration, Instant};


/*#[link(name = "fpgalibrary")]
extern "C" {
    fn initialize() -> * const HardwareCommon;
    fn closeHardware(hc: * const HardwareCommon);
}*/


fn main() {
    // initializes and runs a timely dataflow.
    /*let hwcommon;
    unsafe {
        hwcommon = initialize();
    }*/
    timely::execute_from_args(std::env::args(),  |worker, hc| {

       // let hc;
       // unsafe {
       //     hc = initialize();
       // }

        let index = worker.index();
        let mut input = InputHandle::new();
        let mut probe = ProbeHandle::new();

        // create a new input, exchange data, and inspect its output

        worker.dataflow(|scope| {
            scope.input_from(&mut input)
                 .fpga_wrapper(hc)
                 .inspect(move |x| println!("worker {}:\thello {}", index, x))
                 .probe_with(&mut probe);
        });
	
        // introduce data and watch!
        let start = Instant::now();
        let mut epoch_start = Instant::now();
        let mut hist = hdrhist::HDRHist::new();

        for round in 0..1000 {
            
	    for j in 0..8 {
                input.send(round);// max = 0
	    }
            
            input.advance_to(round + 1);

            while probe.less_than(input.time()) {
                worker.step();
            }
            let epoch_end = Instant::now();
            let epoch_nanos = (epoch_end - epoch_start).as_nanos();
            epoch_start = epoch_end;
            hist.add_value(epoch_nanos as u64);
        }

        let epoch_end = Instant::now();

        let total_nanos = (Instant::now() - start).as_nanos();
        let epoch_latency = (total_nanos as f64) / 1_000_000_000f64 / (1000 as f64); // sec
        let epoch_throughput = (1000 as f64) / (total_nanos as f64) * 1_000_000_000f64; // epochs/sec
	println!("epoch time: {}", epoch_latency); 

        println!("total time (nanos): {}, throughput: {}", total_nanos, epoch_throughput);
        println!("epoch latency (nanos):\n{}", hist.summary_string());

        //unsafe {
        //    closeHardware(hc);
        //}
    }).unwrap();
    /*unsafe {
    	closeHardware(hwcommon);
    }*/
	
}
