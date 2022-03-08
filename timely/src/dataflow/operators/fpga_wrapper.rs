//! Funtionality to run operators on FPGA
pub extern crate libc;

use std::sync::{Arc, Mutex};

use crate::Data;
use crate::dataflow::{Stream, Scope, ScopeParent};
use crate::dataflow::channels::pact::Pipeline;
use crate::dataflow::operators::generic::operator::Operator;
use crate::progress::{Operate, operate::SharedProgress, Timestamp, ChangeBatch, Antichain};
use crate::scheduling::{Schedule, Activations};
use crate::dataflow::operators::generic::builder_raw::OperatorBuilder;
use crate::dataflow::operators::generic::builder_raw::OperatorShape;
use crate::dataflow::channels::pullers::Counter as PullCounter;
use crate::dataflow::channels::pushers::buffer::Buffer as PushBuffer;
use crate::dataflow::channels::pushers::{Counter as PushCounter, Tee};

use std::cell::RefCell;
use std::rc::Rc;
use std::borrow::Borrow;
use std::ops::Deref;
use crate::logging::TimelyEvent::Operates;
use crate::progress::frontier::MutableAntichain;
use std::time::{Duration, Instant};

use std::ptr;

//use std::collections::HashMap;
//#[path = "../../../hardware.rs"]
//pub mod hardware;

use std::ffi::c_void;
use std::collections::HashMap;

#[repr(C)]
/// Data structure to store FPGA related data
pub struct HardwareCommon {
    fd: i32,
    cpid: i32,
    rd_cmd_cnt: u32,
    wr_cmd_cnt: u32,
    cnfg_reg: * mut c_void,
    ctrl_reg: * mut c_void,
    cnfg_reg_avx: * mut c_void,
    hMem: * mut c_void,
    oMem: * mut c_void
}


unsafe impl Send for HardwareCommon{}
unsafe impl Sync for HardwareCommon{}

#[link(name = "fpgalibrary")]
extern "C" {
    fn run(hc: * const HardwareCommon);
}

/// Wrapper operator to store ghost operators
struct FpgaOperator<T, L>
    where
        T: Timestamp,
        L: FnMut(&mut SharedProgress<T>)->bool+'static,
{
    shape: OperatorShape,
    address: Vec<usize>,
    logic: L,
    shared_progress: Rc<RefCell<SharedProgress<T>>>,
    activations: Rc<RefCell<Activations>>,
    summary: Vec<Vec<Antichain<T::Summary>>>,

    ghost_indexes: Vec<(usize, usize)>,
}
impl<T, L> Schedule for FpgaOperator<T, L>
    where
        T: Timestamp,
        L: FnMut(&mut SharedProgress<T>)->bool+'static,
{
    fn name(&self) -> &str { self.shape.name()}
    fn path(&self) -> &[usize] { &self.address[..] }
    fn schedule(&mut self) -> bool {
        let shared_progress = &mut *self.shared_progress.borrow_mut();
        (self.logic)(shared_progress)
    }
}

impl<T, L> Operate<T> for FpgaOperator<T, L>
    where
        T: Timestamp,
        L: FnMut(&mut SharedProgress<T>)->bool+'static,
{
    fn inputs(&self) -> usize { self.shape.inputs() }
    fn outputs(&self) -> usize { self.shape.outputs() }

    // we need a different get_internal_summary function for FpgaOperator, as we need to use wrapper_internals
    // to pass initial frontier values to each ghost operator
    fn get_internal_summary(&mut self) -> (Vec<Vec<Antichain<T::Summary>>>, Rc<RefCell<SharedProgress<T>>>) {

        // Request the operator to be scheduled at least once.
        self.activations.borrow_mut().activate(&self.address[..]);

        // by default, we reserve a capability for each output port at `Default::default()`.
        for (i, j) in self.ghost_indexes.iter() {
            self.shared_progress
                .borrow_mut()
                .wrapper_internals.get_mut(j).unwrap()
                .iter_mut()
                .for_each(|output| output.update(T::minimum(), self.shape.peers() as i64));
        }


        (self.summary.clone(), self.shared_progress.clone())
    }

    // initialize self.frontier antichains as indicated by hosting scope.
    fn set_external_summary(&mut self) {
        // should we schedule the operator here, or just await the first invocation?
        self.schedule();
    }

    fn notify_me(&self) -> bool { self.shape.notify() }
}

/// Ghost operator, resides on the FPGA side
struct FakeOperator<T, L>
    where
        T: Timestamp,
        L: FnMut(&mut SharedProgress<T>)->bool+'static,
{
    shape: OperatorShape,
    address: Vec<usize>,
    logic: L,
    shared_progress: Rc<RefCell<SharedProgress<T>>>,
    activations: Rc<RefCell<Activations>>,
    summary: Vec<Vec<Antichain<T::Summary>>>,
}

impl<T, L> Schedule for FakeOperator<T, L>
    where
        T: Timestamp,
        L: FnMut(&mut SharedProgress<T>)->bool+'static,
{
    fn name(&self) -> &str { self.shape.name()}
    fn path(&self) -> &[usize] { &self.address[..] }

    // we need to return false from this function in case of ghost operator.
    fn schedule(&mut self) -> bool {
        let shared_progress = &mut *self.shared_progress.borrow_mut();
        (self.logic)(shared_progress);
        false
    }
}

//progress is extracted from operatr state

impl<T, L> Operate<T> for FakeOperator<T, L>
    where
        T: Timestamp,
        L: FnMut(&mut SharedProgress<T>)->bool+'static,
{
    fn inputs(&self) -> usize { self.shape.inputs() }
    fn outputs(&self) -> usize { self.shape.outputs() }

    // announce internal topology as fully connected, and hold all default capabilities.
    fn get_internal_summary(&mut self) -> (Vec<Vec<Antichain<T::Summary>>>, Rc<RefCell<SharedProgress<T>>>) {

        // we don't need to activate ghost operator
        //self.activations.borrow_mut().activate(&self.address[..]);

        // by default, we reserve a capability for each output port at `Default::default()`.
        self.shared_progress
            .borrow_mut()
            .internals
            .iter_mut()
            .for_each(|output| output.update(T::minimum(), self.shape.peers() as i64));

        // we have summaries as nodes in rachability builder, so there should be a correct summery for this node as well
        let connection = vec![Antichain::from_elem(Default::default()); 0];
        self.summary.push(connection);

        let connection2 = vec![Antichain::from_elem(Default::default()); 1];
        for (summary, entry) in self.summary.iter_mut().zip(connection2.into_iter()) {
            summary.push(entry);
        }
        (self.summary.clone(), self.shared_progress.clone())
    }

    // initialize self.frontier antichains as indicated by hosting scope.
    fn set_external_summary(&mut self) {
        // should we schedule the operator here, or just await the first invocation?
        //self.schedule();
    }

    fn notify_me(&self) -> bool { self.shape.notify() }
}

/// Wrapper to run on FPGA
pub trait FpgaWrapper<S: Scope/*, D: Data*/> {

    /// Wrapper function
    fn fpga_wrapper(&self, hc: *const HardwareCommon) -> Stream<S, u64>;

}

// return value should be the value of the last operator

impl<S: Scope<Timestamp = u64>> FpgaWrapper<S> for Stream<S, u64> {


    fn fpga_wrapper(&self, hc: *const HardwareCommon) -> Stream<S, u64> {

        // this should correspond to the way the data will be read on the fpga
        let mut ghost_indexes = Vec::new();
        let mut ghost_indexes2 = Vec::new();
        // TODO: should get rid of ghost indexes
        let mut current_index = 0;

        // CREATE FILTER GHOST OPERATOR 1
        /*let mut builder_filter = OperatorBuilder::new("Filter".to_owned(), self.scope()); // scope comes from stream
        builder_filter.set_notify(false);
        builder_filter.set_shape(1, 1);

        let operator_logic_filter =
         move |progress: &mut SharedProgress<S::Timestamp>| { false};

        let operator_filter = FakeOperator {
            shape: builder_filter.shape().clone(),
            address: builder_filter.address().clone(),
            activations: self.scope().activations().clone(),
            logic: operator_logic_filter,
            shared_progress: Rc::new(RefCell::new(SharedProgress::new(1, 1))),
            summary: builder_filter.summary().to_vec(),
        };


        self.scope().add_operator_with_indices_no_path(Box::new(operator_filter), builder_filter.index(), builder_filter.global());
        ghost_indexes.push((current_index, builder_filter.index()));
        ghost_indexes2.push((current_index, builder_filter.index()));
        current_index += 1;*/

        // CREATE FILTER GHOST OPERATOR 1
        let mut builder_filter1 = OperatorBuilder::new("Filter1".to_owned(), self.scope()); // scope comes from stream
        builder_filter1.set_notify(false);
        builder_filter1.set_shape(1, 1);

        let operator_logic_filter1 =
         move |progress: &mut SharedProgress<S::Timestamp>| { false};

        let operator_filter1 = FakeOperator {
            shape: builder_filter1.shape().clone(),
            address: builder_filter1.address().clone(),
            activations: self.scope().activations().clone(),
            logic: operator_logic_filter1,
            shared_progress: Rc::new(RefCell::new(SharedProgress::new(1, 1))),
            summary: builder_filter1.summary().to_vec(),
        };


        self.scope().add_operator_with_indices_no_path(Box::new(operator_filter1), builder_filter1.index(), builder_filter1.global());
        ghost_indexes.push((current_index, builder_filter1.index()));
        ghost_indexes2.push((current_index, builder_filter1.index()));
        current_index += 1;


        // CREATE FILTER GHOST OPERATOR 2
        let mut builder_filter2 = OperatorBuilder::new("Filter2".to_owned(), self.scope()); // scope comes from stream
        builder_filter2.set_notify(false);
        builder_filter2.set_shape(1, 1);

        let operator_logic_filter2 =
         move |progress: &mut SharedProgress<S::Timestamp>| { false};

        let operator_filter2 = FakeOperator {
            shape: builder_filter2.shape().clone(),
            address: builder_filter2.address().clone(),
            activations: self.scope().activations().clone(),
            logic: operator_logic_filter2,
            shared_progress: Rc::new(RefCell::new(SharedProgress::new(1, 1))),
            summary: builder_filter2.summary().to_vec(),
        };


        self.scope().add_operator_with_indices_no_path(Box::new(operator_filter2), builder_filter2.index(), builder_filter2.global());
        ghost_indexes.push((current_index, builder_filter2.index()));
        ghost_indexes2.push((current_index, builder_filter2.index()));
        current_index += 1;

        // CREATE FILTER GHOST OPERATOR 3
        let mut builder_filter3 = OperatorBuilder::new("Filter3".to_owned(), self.scope()); // scope comes from stream
        builder_filter3.set_notify(false);
        builder_filter3.set_shape(1, 1);

        let operator_logic_filter3 =
         move |progress: &mut SharedProgress<S::Timestamp>| { false};

        let operator_filter3 = FakeOperator {
            shape: builder_filter3.shape().clone(),
            address: builder_filter3.address().clone(),
            activations: self.scope().activations().clone(),
            logic: operator_logic_filter3,
            shared_progress: Rc::new(RefCell::new(SharedProgress::new(1, 1))),
            summary: builder_filter3.summary().to_vec(),
        };


        self.scope().add_operator_with_indices_no_path(Box::new(operator_filter3), builder_filter3.index(), builder_filter3.global());
        ghost_indexes.push((current_index, builder_filter3.index()));
        ghost_indexes2.push((current_index, builder_filter3.index()));
        current_index += 1;

        // CREATE FILTER GHOST OPERATOR 4
        let mut builder_filter4 = OperatorBuilder::new("Filter4".to_owned(), self.scope()); // scope comes from stream
        builder_filter4.set_notify(false);
        builder_filter4.set_shape(1, 1);

        let operator_logic_filter4 =
         move |progress: &mut SharedProgress<S::Timestamp>| { false};

        let operator_filter4 = FakeOperator {
            shape: builder_filter4.shape().clone(),
            address: builder_filter4.address().clone(),
            activations: self.scope().activations().clone(),
            logic: operator_logic_filter4,
            shared_progress: Rc::new(RefCell::new(SharedProgress::new(1, 1))),
            summary: builder_filter4.summary().to_vec(),
        };


        self.scope().add_operator_with_indices_no_path(Box::new(operator_filter4), builder_filter4.index(), builder_filter4.global());
        ghost_indexes.push((current_index, builder_filter4.index()));
        ghost_indexes2.push((current_index, builder_filter4.index()));
        current_index += 1;


        // CREATE FILTER GHOST OPERATOR 5
        let mut builder_filter5 = OperatorBuilder::new("Filter5".to_owned(), self.scope()); // scope comes from stream
        builder_filter5.set_notify(false);
        builder_filter5.set_shape(1, 1);

        let operator_logic_filter5 =
         move |progress: &mut SharedProgress<S::Timestamp>| { false};

        let operator_filter5 = FakeOperator {
            shape: builder_filter5.shape().clone(),
            address: builder_filter5.address().clone(),
            activations: self.scope().activations().clone(),
            logic: operator_logic_filter5,
            shared_progress: Rc::new(RefCell::new(SharedProgress::new(1, 1))),
            summary: builder_filter5.summary().to_vec(),
        };


        self.scope().add_operator_with_indices_no_path(Box::new(operator_filter5), builder_filter5.index(), builder_filter5.global());
        ghost_indexes.push((current_index, builder_filter5.index()));
        ghost_indexes2.push((current_index, builder_filter5.index()));
        current_index += 1;


        // CREATE FILTER GHOST OPERATOR 6
        let mut builder_filter6 = OperatorBuilder::new("Filter6".to_owned(), self.scope()); // scope comes from stream
        builder_filter6.set_notify(false);
        builder_filter6.set_shape(1, 1);

        let operator_logic_filter6 =
         move |progress: &mut SharedProgress<S::Timestamp>| { false};

        let operator_filter6 = FakeOperator {
            shape: builder_filter6.shape().clone(),
            address: builder_filter6.address().clone(),
            activations: self.scope().activations().clone(),
            logic: operator_logic_filter6,
            shared_progress: Rc::new(RefCell::new(SharedProgress::new(1, 1))),
            summary: builder_filter6.summary().to_vec(),
        };


        self.scope().add_operator_with_indices_no_path(Box::new(operator_filter6), builder_filter6.index(), builder_filter6.global());
        ghost_indexes.push((current_index, builder_filter6.index()));
        ghost_indexes2.push((current_index, builder_filter6.index()));
        current_index += 1;


        // CREATE FILTER GHOST OPERATOR 7
        let mut builder_filter7 = OperatorBuilder::new("Filter7".to_owned(), self.scope()); // scope comes from stream
        builder_filter7.set_notify(false);
        builder_filter7.set_shape(1, 1);

        let operator_logic_filter7 =
         move |progress: &mut SharedProgress<S::Timestamp>| { false};

        let operator_filter7 = FakeOperator {
            shape: builder_filter7.shape().clone(),
            address: builder_filter7.address().clone(),
            activations: self.scope().activations().clone(),
            logic: operator_logic_filter7,
            shared_progress: Rc::new(RefCell::new(SharedProgress::new(1, 1))),
            summary: builder_filter7.summary().to_vec(),
        };


        self.scope().add_operator_with_indices_no_path(Box::new(operator_filter7), builder_filter7.index(), builder_filter7.global());
        ghost_indexes.push((current_index, builder_filter7.index()));
        ghost_indexes2.push((current_index, builder_filter7.index()));
        current_index += 1;


        // CREATE FILTER GHOST OPERATOR 8
        let mut builder_filter8 = OperatorBuilder::new("Filter8".to_owned(), self.scope()); // scope comes from stream
        builder_filter8.set_notify(false);
        builder_filter8.set_shape(1, 1);

        let operator_logic_filter8 =
         move |progress: &mut SharedProgress<S::Timestamp>| { false};

        let operator_filter8 = FakeOperator {
            shape: builder_filter8.shape().clone(),
            address: builder_filter8.address().clone(),
            activations: self.scope().activations().clone(),
            logic: operator_logic_filter8,
            shared_progress: Rc::new(RefCell::new(SharedProgress::new(1, 1))),
            summary: builder_filter8.summary().to_vec(),
        };


        self.scope().add_operator_with_indices_no_path(Box::new(operator_filter8), builder_filter8.index(), builder_filter8.global());
        ghost_indexes.push((current_index, builder_filter8.index()));
        ghost_indexes2.push((current_index, builder_filter8.index()));
        current_index += 1;


        // CREATE FILTER GHOST OPERATOR 9
        let mut builder_filter9 = OperatorBuilder::new("Filter9".to_owned(), self.scope()); // scope comes from stream
        builder_filter9.set_notify(false);
        builder_filter9.set_shape(1, 1);

        let operator_logic_filter9 =
         move |progress: &mut SharedProgress<S::Timestamp>| { false};

        let operator_filter9 = FakeOperator {
            shape: builder_filter9.shape().clone(),
            address: builder_filter9.address().clone(),
            activations: self.scope().activations().clone(),
            logic: operator_logic_filter9,
            shared_progress: Rc::new(RefCell::new(SharedProgress::new(1, 1))),
            summary: builder_filter9.summary().to_vec(),
        };


        self.scope().add_operator_with_indices_no_path(Box::new(operator_filter9), builder_filter9.index(), builder_filter9.global());
        ghost_indexes.push((current_index, builder_filter9.index()));
        ghost_indexes2.push((current_index, builder_filter9.index()));
        current_index += 1;

        // CREATE FILTER GHOST OPERATOR 10
        let mut builder_filter10 = OperatorBuilder::new("Filter10".to_owned(), self.scope()); // scope comes from stream
        builder_filter10.set_notify(false);
        builder_filter10.set_shape(1, 1);

        let operator_logic_filter10 =
         move |progress: &mut SharedProgress<S::Timestamp>| { false};

        let operator_filter10 = FakeOperator {
            shape: builder_filter10.shape().clone(),
            address: builder_filter10.address().clone(),
            activations: self.scope().activations().clone(),
            logic: operator_logic_filter10,
            shared_progress: Rc::new(RefCell::new(SharedProgress::new(1, 1))),
            summary: builder_filter10.summary().to_vec(),
        };


        self.scope().add_operator_with_indices_no_path(Box::new(operator_filter10), builder_filter10.index(), builder_filter10.global());
        ghost_indexes.push((current_index, builder_filter10.index()));
        ghost_indexes2.push((current_index, builder_filter10.index()));
        current_index += 1;


        // CREATE MAP GHOST OPERATOR
        let mut builder_map = OperatorBuilder::new("Map".to_owned(), self.scope()); // scope comes from stream
        builder_map.set_notify(false);
        builder_map.set_shape(1, 1);

        let operator_logic_map =
            move |progress: &mut SharedProgress<S::Timestamp>| { false};

        let operator_map = FakeOperator {
            shape: builder_map.shape().clone(),
            address: builder_map.address().clone(),
            activations: self.scope().activations().clone(),
            logic: operator_logic_map,
            shared_progress: Rc::new(RefCell::new(SharedProgress::new(1, 1))),
            summary: builder_map.summary().to_vec(),
        };


        self.scope().add_operator_with_indices_no_path(Box::new(operator_map), builder_map.index(), builder_map.global());
        ghost_indexes.push((current_index, builder_map.index()));
        ghost_indexes2.push((current_index, builder_map.index()));
        current_index += 1;

        // CREATE AGGREGATE AGGREGATE OPERATOR
        let mut builder_aggregate = OperatorBuilder::new("Aggregate".to_owned(), self.scope()); // scope comes from stream
        builder_aggregate.set_notify(false);
        builder_aggregate.set_shape(1, 1);

        let operator_logic_aggregate =
            move |progress: &mut SharedProgress<S::Timestamp>| { false};

        let operator_aggregate = FakeOperator {
            shape: builder_aggregate.shape().clone(),
            address: builder_aggregate.address().clone(),
            activations: self.scope().activations().clone(),
            logic: operator_logic_aggregate,
            shared_progress: Rc::new(RefCell::new(SharedProgress::new(1, 1))),
            summary: builder_aggregate.summary().to_vec(),
        };


        self.scope().add_operator_with_indices_no_path(Box::new(operator_aggregate), builder_aggregate.index(), builder_aggregate.global());
        ghost_indexes.push((current_index, builder_aggregate.index()));
        ghost_indexes2.push((current_index, builder_aggregate.index()));
        current_index += 1;
        
        // create wrapper operator

        let mut builder_wrapper = OperatorBuilder::new("Wrapper".to_owned(), self.scope()); // scope comes from stream
        let mut input_wrapper = PullCounter::new(builder_wrapper.new_input(self, Pipeline)); // builder.new_input -> creates new Input and new input connection in builder_raw.rs
        let (tee_wrapper, stream_wrapper) = builder_wrapper.new_output();
        // this stream is returned every time, Rust will probably complain.
        // create new_output_connection function without returning the stream?
        let mut output_wrapper = PushBuffer::new(PushCounter::new(tee_wrapper));


        let frontier = Rc::new(RefCell::new(vec![MutableAntichain::new(); ghost_indexes.len()]));
        let mut started = false;

        let mut vector = Vec::with_capacity(8192);
        let mut vector2 = Vec::with_capacity(8192);

        let mut produced = HashMap::with_capacity(32);
        let mut consumed = HashMap::with_capacity(32);
        let mut internals = HashMap::with_capacity(32);


        let raw_logic =
            move |progress: &mut SharedProgress<S::Timestamp>| {

                let start1 = Instant::now();
                let mut borrow = frontier.borrow_mut();

                for (i, j) in ghost_indexes.iter() {
                    borrow[*i].update_iter(progress.wrapper_frontiers.get_mut(j).unwrap()[0].drain());
                }

                if !started {
                    // discard initial capability.
                    for (i, j) in ghost_indexes.iter() {
                        progress.wrapper_internals.get_mut(j).unwrap()[0].update(S::Timestamp::minimum(), -1);
                        started = true;
                    }
                }


                // invoke supplied logic
                use crate::communication::message::RefOrMut;

                let param = 500; // number of 8 number chuncks
                let param_output = 1;
                let frontier_param = 2;
                let mut has_data = false;
                /*let end1 = Instant::now();
                let delta1 = (end1 - start1).as_nanos();
                println!("Delta1 = {}", delta1);*/
                
                while let Some(message) = input_wrapper.next() {
                    has_data = true;
                    //println!("INSIDE DATA PROCESSING");
                    let (time, data) = match message.as_ref_or_mut() {
                        RefOrMut::Ref(reference) => (&reference.time, RefOrMut::Ref(&reference.data)),
                        RefOrMut::Mut(reference) => (&reference.time, RefOrMut::Mut(&mut reference.data)),
                    };
                    data.swap(&mut vector);

                    let mut frontier_length =  frontier_param * 8;//2 + ghost_indexes.len() + 4 * ghost_indexes.len();
                    let mut current_length = 0;
                    let mut max_length = param * 8 + frontier_param * 8;
                    let mut data_length = param * 8;
                    let mut data_start_index = 0;
                    let mut progress_start_index = param_output * 8;

                    unsafe {

                        //let start2 = Instant::now();
                        let memory = (*hc).hMem as *mut u64;
                        *memory.offset(current_length as isize) = *time ;
                        current_length += 1;

                        for i in 0 .. borrow.len() {
                            let frontier = borrow[i].borrow().frontier();
			                if frontier.len() == 0 {
                                *memory.offset(current_length as isize) = 0;
			    	            current_length += 1;
			                } else {
                                for val in frontier.iter() {
                                    *memory.offset(current_length as isize) = (*val << 1) | 1u64;
                                    current_length += 1;
                            	}
			                }
                        }
                        for i in current_length .. frontier_length {
                            *memory.offset(current_length as isize) = 0;
                            current_length += 1;
                        }

			            if vector.len() == 0 {
                            *memory.offset(current_length as isize) = 0 as u64;
                            current_length += 1;
			            } else {
                       	    for val in vector.iter() {
                                *memory.offset(current_length as isize) = ((*val << 1) | 1u64) as u64;
                                current_length += 1;
                            }
			            }

                        for i in current_length .. max_length {
                            *memory.offset(i as isize) = 0;
                        }
                        /*let end2 = Instant::now();
                        let delta2 = (end2 - start2).as_nanos();
                        println!("Delta2 = {}", delta2);*/
                        
                        //for (i, elem) in vector.iter().enumerate() {
                            //println!("{} input element = {}", i, *elem);
                        //}

                        //println!("all_length = {}", all_length);
                        //println!("current_length = {}", current_length);
                        //println!("data_start_index = {}", data_start_index);
                        //println!("progress_start_index = {}", progress_start_index);
 
			            /*println!("PRINT INPUT VECTOR TO FPGA");
			            for elem in input_vector.iter() {
				        print!(" {}", elem);
			            }

			            println!();*/

                        //let start3 = Instant::now();

                        run(hc);// changes should be reflected in hc
                        
                        /*let end3 = Instant::now();
                        let delta3 = (end3 - start3).as_nanos();
                        println!("Delta3 = {}", delta3);*/
                        
                        /*println!("PRINT OUTPUT VECTOR FROM FPGA");
                        for (i, elem) in output.iter().enumerate() {
                            print!(" {}", elem);
                        }
                        println!();*/

                        //let start4 = Instant::now();
                        let memory_out = (*hc).oMem as *mut u64;
                        //let pointer_in = memory_out.offset(0 as isize);
                        //let pointer_out = vector2.as_mut_ptr();

                        //ptr::copy_nonoverlapping(pointer_in, pointer_out, data_length);

                        //vector2.set_len(data_length);

                        for i in 0 .. data_length {
                            let val = *memory_out.offset(i as isize) as u64;
                            let shifted_val = val >> 1;
                            if val != 0 {
                                vector2.push(shifted_val);
                            }
                        }

                        /*let end4 = Instant::now();
                        let delta4 = (end4 - start4).as_nanos();
                        println!("Delta4 = {}", delta4);*/
                        

                        //vector2.push(1);
                        //let start5 = Instant::now();
                        for (i, j) in ghost_indexes.iter() {
                            //println!("consumed = {}", output[progress_start_index + 4*i]);
                            //println!("internal time = {}", output[progress_start_index + 4*i + 2] >> 1);
                            //println!("internal update {}", output[progress_start_index + 4*i + 3] as i64);
                            //println!("produced = {}", output[progress_start_index + 4*i + 1]);
                            let consumed_index = (progress_start_index + 4*i) as isize;
                            let produced_index = (progress_start_index + 4*i + 1) as isize;
                            let internals_index_1 = (progress_start_index + 4*i + 2) as isize;
                            let internals_index_2 = (progress_start_index + 4*i + 3) as isize;

                            let consumed_value = *memory_out.offset(consumed_index) as i64;
                            let produced_value = *memory_out.offset(produced_index) as i64;
                            let internals_time = *memory_out.offset(internals_index_1)  >> 1 as u64;
                            let internals_value = *memory_out.offset(internals_index_2) as i64;


                            /*println!("consumed index = {}", consumed_index);
                            println!("produced index = {}", produced_index);

                            println!("consumed = {}", consumed_value);
                            println!("produced = {}", produced_value);
                            println!("internal time = {}", internals_time);
                            println!("internal update {}", internals_value);*/

                            consumed.insert(*j, consumed_value);
                            internals.insert(*j, (internals_time, internals_value));
                            produced.insert(*j, produced_value);

                        }
                        /*let end5 = Instant::now();
                        let delta5 = (end5 - start5).as_nanos();
                        println!("Delta5 = {}", delta5);*/
                    }


                    //let start6 = Instant::now();
                    output_wrapper.session(time).give_vec(&mut vector2);

                    for (i, j) in ghost_indexes.iter() {
                        let mut cb = ChangeBatch::new_from(time.clone(), *consumed.get(j).unwrap());
                        let mut cb1 = ChangeBatch::new_from(time.clone(), *produced.get(j).unwrap());
 			            let mut cb2 = ChangeBatch::new_from(internals.get(j).unwrap().0 as u64, internals.get(j).unwrap().1 as i64);
                        cb.drain_into(&mut progress.wrapper_consumeds.get_mut(j).unwrap()[0]);
                        cb1.drain_into(&mut progress.wrapper_produceds.get_mut(j).unwrap()[0]);
                        cb2.drain_into(&mut progress.wrapper_internals.get_mut(j).unwrap()[0]);
                    }

                    /*let end6 = Instant::now();
                    let delta6 = (end6 - start6).as_nanos();
                    println!("Delta6 = {}", delta6);*/

                }
                
                if !has_data {
		            //println!("no data");

                    let mut frontier_length = frontier_param * 8;//2 + ghost_indexes.len() + 4 * ghost_indexes.len();
                    let mut current_length = 0;
                    let mut max_length = param * 8 + frontier_param * 8;
                    let mut data_length = param * 8;
                    let mut data_start_index = 0;
                    let mut progress_start_index = param_output * 8;

                    unsafe {

                        let memory = (*hc).hMem as *mut u64;
                        *memory.offset(current_length as isize) = 0;
                        current_length += 1;

                        for i in 0 .. borrow.len() {
                            let frontier = borrow[i].borrow().frontier();
			                if frontier.len() == 0 {
                                *memory.offset(current_length as isize) = 0;
                                current_length += 1;
                            } else {
                                for val in frontier.iter() {
                                    *memory.offset(current_length as isize) = (*val << 1) | 1u64;
                                    current_length += 1;
                                }
			                }
                        }



                        for i in current_length .. max_length {
                            *memory.offset(i as isize) = 0;
                        }

			            //println!("PRINT INPUT VECTOR TO FPGA");
			            //for elem in input_vector.iter() {
			            //	print!(" {}", elem);
			            //}
			            //println!();

			            run(hc);

			            /*println!("PRINT OUTPUT VECTOR FROM FPGA");
                        for (i, elem) in output.iter().enumerate() {
                            print!(" {}", elem);
                        }
			            println!();*/

                        let memory_out = (*hc).oMem as *mut u64;

                        for i in 0 .. data_length {
                            let val = *memory_out.offset(i as isize) as u64;
                            let shifted_val = val >> 1;
                            if val != 0 {
                                vector2.push(shifted_val);
                            }
                        }
                        //vector2.push(1);
                        for (i, j) in ghost_indexes.iter() {
                            let consumed_index = (progress_start_index + 4*i) as isize;
                            let produced_index = (progress_start_index + 4*i + 1) as isize;
                            let internals_index_1 = (progress_start_index + 4*i + 2) as isize;
                            let internals_index_2 = (progress_start_index + 4*i + 3) as isize;

                            let consumed_value = *memory_out.offset(consumed_index) as i64;
                            let produced_value = *memory_out.offset(produced_index) as i64;
                            let internals_time = *memory_out.offset(internals_index_1)  >> 1 as u64;
                            let internals_value = *memory_out.offset(internals_index_2) as i64;

                            /*println!("consumed = {}", consumed_value);
                            println!("produced = {}", produced_value);
                            println!("internal time = {}", internals_time);
                            println!("internal update {}", internals_value);*/


                            consumed.insert(*j, consumed_value);
                            internals.insert(*j, (internals_time, internals_value));
                            produced.insert(*j, produced_value);


                        }
                    }
		            let id_wrap = ghost_indexes[ghost_indexes.len() - 1].1;
		            //println!("********id wrap********** = {}", id_wrap);

		            if vector2.len() > 0 {
		                output_wrapper.session(&(internals.get(&id_wrap).unwrap().0 as u64)).give_vec(&mut vector2);

                        let mut cb1 = ChangeBatch::new_from(internals.get(&id_wrap).unwrap().0 as u64, *produced.get(&id_wrap).unwrap());
 		                let mut cb2 = ChangeBatch::new_from(internals.get(&id_wrap).unwrap().0 as u64, internals.get(&id_wrap).unwrap().1 as i64);
                        cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&id_wrap).unwrap()[0]);
                        cb2.drain_into(&mut progress.wrapper_internals.get_mut(&id_wrap).unwrap()[0]);
		            }
  	 	        }

                //let start7 = Instant::now();
                vector.clear();
                vector2.clear();
                produced.clear();
                consumed.clear();
                internals.clear();
                output_wrapper.cease();
                /*let end7 = Instant::now();
                let delta7 = (end7 - start7).as_nanos();
                println!("Delta1 = {}", delta7);*/

                false
            };

        let mut ghost_operators = Vec::new();
        let mut ghost_edges = Vec::new();
        // when we have multiple operators we should push edges to ghost edges
        let mut prev_ghost = 0;
        for ghost in ghost_indexes2.iter() {

            if ghost.0 > 0 {
                ghost_edges.push((prev_ghost, ghost.1));
            }
            prev_ghost = ghost.1;
        }
        ghost_operators.push(builder_filter1.index());
        ghost_operators.push(builder_filter2.index());
        ghost_operators.push(builder_filter3.index());
        ghost_operators.push(builder_filter4.index());
        ghost_operators.push(builder_filter5.index());
        ghost_operators.push(builder_filter6.index());
        ghost_operators.push(builder_filter7.index());
        ghost_operators.push(builder_filter8.index());
        ghost_operators.push(builder_filter9.index());
        ghost_operators.push(builder_filter10.index());
        /*ghost_operators.push(builder_filter11.index());
        ghost_operators.push(builder_filter12.index());
        ghost_operators.push(builder_filter13.index());
        ghost_operators.push(builder_filter14.index());
        ghost_operators.push(builder_filter15.index());
        ghost_operators.push(builder_filter16.index());
        ghost_operators.push(builder_filter17.index());
        ghost_operators.push(builder_filter18.index());
        ghost_operators.push(builder_filter19.index());
        ghost_operators.push(builder_filter20.index());*/
        ghost_operators.push(builder_map.index());
        ghost_operators.push(builder_aggregate.index());

        builder_wrapper.set_notify(false);
        let operator = FpgaOperator {
            shape: builder_wrapper.shape().clone(),
            address: builder_wrapper.address().clone(),
            activations: self.scope().activations().clone(),
            logic: raw_logic,
            shared_progress: Rc::new(RefCell::new(SharedProgress::new_ghosts(builder_wrapper.shape().inputs(), builder_wrapper.shape().outputs(), ghost_operators.clone()))),
            summary: builder_wrapper.summary().to_vec(),
            ghost_indexes: ghost_indexes2
        };

        self.scope().add_operator_with_indices(Box::new(operator), builder_wrapper.index(), builder_wrapper.global());



        // we also need to create a map from ghost to wrapper

        self.scope().add_fpga_operator(builder_wrapper.index(), ghost_operators, ghost_edges);


        return stream_wrapper;
    }
}
