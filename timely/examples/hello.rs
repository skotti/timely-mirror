extern crate timely;

use timely::dataflow::{InputHandle, ProbeHandle};
use timely::dataflow::operators::{Input, Exchange, Inspect, Probe, FpgaWrapper};
use timely::dataflow::operators::fpga_wrapper::HardwareCommon;
//#[path = "../hardware.rs"]
//pub mod hardware;


#[link(name = "fpgalibrary")]
extern "C" {
    fn initialize() -> * mut HardwareCommon;
    fn closeHardware(hc: * mut HardwareCommon);
}


fn main() {
    // initializes and runs a timely dataflow.
    timely::execute_from_args(std::env::args(), |worker| {

        let hc;
        unsafe {
            hc = initialize();
        }

        let index = worker.index();
        let mut input = InputHandle::new();
        let mut probe = ProbeHandle::new();

        // create a new input, exchange data, and inspect its output
        worker.dataflow(|scope| {
            scope.input_from(&mut input)
                 .exchange(|x| *x)
                 .inspect(move |x| println!("worker {}:\thello {}", index, x))
                 .fpga_wrapper(hc)
                 .inspect(move |x| println!("worker {}:\thello {}", index, x))
                 .probe_with(&mut probe);
        });

        // introduce data and watch!
        for round in 0..10 {
            if index == 0 {
                input.send(round);
            }
            input.advance_to(round + 1);
            while probe.less_than(input.time()) {
                println!("{}", round);
                worker.step();
            }
        }

        unsafe {
            closeHardware(hc);
        }
    }).unwrap();
}
