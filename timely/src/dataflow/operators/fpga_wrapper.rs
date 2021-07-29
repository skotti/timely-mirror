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
    cnfg_reg: *mut c_void,
    ctrl_reg: *mut c_void,
    cnfg_reg_avx: *mut c_void,
    hMem: *mut c_void,
    memory: *mut c_void
}

unsafe impl Send for HardwareCommon {}

unsafe impl Sync for HardwareCommon {}

#[link(name = "fpgalibrary")]
extern "C" {
    fn send_input(hc: *const HardwareCommon, input: *mut u64);
    fn send_input_and_check(hc: *const HardwareCommon, input: *mut u64, output: *mut i64) -> bool;
    fn checkOutput(hc: *const HardwareCommon, output: *mut i64) -> bool;
}

/// Wrapper operator to store ghost operators
struct FpgaOperator<T, L>
    where
        T: Timestamp,
        L: FnMut(&mut SharedProgress<T>) -> bool + 'static,
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
        L: FnMut(&mut SharedProgress<T>) -> bool + 'static,
{
    fn name(&self) -> &str { self.shape.name() }
    fn path(&self) -> &[usize] { &self.address[..] }
    fn schedule(&mut self) -> bool {
        let shared_progress = &mut *self.shared_progress.borrow_mut();
        (self.logic)(shared_progress)
    }
}

impl<T, L> Operate<T> for FpgaOperator<T, L>
    where
        T: Timestamp,
        L: FnMut(&mut SharedProgress<T>) -> bool + 'static,
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
        L: FnMut(&mut SharedProgress<T>) -> bool + 'static,
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
        L: FnMut(&mut SharedProgress<T>) -> bool + 'static,
{
    fn name(&self) -> &str { self.shape.name() }
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
        L: FnMut(&mut SharedProgress<T>) -> bool + 'static,
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

impl<S: Scope<Timestamp=u64>> FpgaWrapper<S> for Stream<S, u64> {
    fn fpga_wrapper(&self, hc: *const HardwareCommon) -> Stream<S, u64> {

        // this should correspond to the way the data will be read on the fpga
        let mut ghost_indexes = Vec::new();
        let mut ghost_indexes2 = Vec::new();
        // TODO: should get rid of ghost indexes
        let mut current_index = 0;

        // CREATE FILTER GHOST OPERATOR
        let mut builder_filter = OperatorBuilder::new("Filter".to_owned(), self.scope()); // scope comes from stream
        builder_filter.set_notify(false);
        builder_filter.set_shape(1, 1);

        let operator_logic_filter =
            move |progress: &mut SharedProgress<S::Timestamp>| { false };

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
        current_index += 1;


        // CREATE MAP GHOST OPERATOR
        let mut builder_map = OperatorBuilder::new("Map".to_owned(), self.scope()); // scope comes from stream
        builder_map.set_notify(false);
        builder_map.set_shape(1, 1);

        let operator_logic_map =
            move |progress: &mut SharedProgress<S::Timestamp>| { false };

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

        // CREATE AGGREGATE GHOST OPERATOR
        let mut builder_aggregate = OperatorBuilder::new("Aggregate".to_owned(), self.scope()); // scope comes from stream
        builder_aggregate.set_notify(false);
        builder_aggregate.set_shape(1, 1);

        let operator_logic_aggregate =
            move |progress: &mut SharedProgress<S::Timestamp>| { false };

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

        let raw_logic =
            move |progress: &mut SharedProgress<S::Timestamp>| {
                let start1 = Instant::now();
                //println! ("SHEDULED!!!");

                let mut borrow = frontier.borrow_mut();

                for (i, j) in ghost_indexes.iter() {
                    borrow[*i].update_iter(progress.wrapper_frontiers.get_mut(j).unwrap()[0].drain());
                }
                //borrow.update_iter(progress.frontiers[0].drain()); // drain = removes elements from vector

                if !started {
                    // discard initial capability.
                    for (i, j) in ghost_indexes.iter() {
                        progress.wrapper_internals.get_mut(j).unwrap()[0].update(S::Timestamp::minimum(), -1);
                        started = true;
                    }
                }


                // invoke supplied logic
                use crate::communication::message::RefOrMut;

                let mut vector = Vec::new();
                let mut input_vector = Vec::new();
                let mut vector2 = Vec::new();

                let mut has_data = false;

                while let Some(message) = input_wrapper.next() {
                    has_data = true;
                    //println!("INSIDE DATA PROCESSING");
                    let (time, data) = match message.as_ref_or_mut() {
                        RefOrMut::Ref(reference) => (&reference.time, RefOrMut::Ref(&reference.data)),
                        RefOrMut::Mut(reference) => (&reference.time, RefOrMut::Mut(&mut reference.data)),
                    };
                    data.swap(&mut vector);
                    // I should call my fpga function here with vector as an input

                    let mut produced = HashMap::new();
                    let mut consumed = HashMap::new();
                    let mut internals = HashMap::new();
                    let mut info_length = 24;//2 + ghost_indexes.len() + 4 * ghost_indexes.len();
                    let mut all_length = 4;//(((info_length + vector.len()) / 8) + 1) as i64;
                    let mut current_length = 0;
                    let mut max_length = 32;
                    let mut data_start_index = 24;
                    let mut progress_start_index = 0;
                    let mut got_output = false;

                    unsafe {

                        current_length += 1;
                        //data_start_index += 1;
                        progress_start_index += 1;
                        input_vector.push(all_length as u64);
                        //println!("increase length");

                        current_length += 1;
                        //data_start_index += 1;
                        progress_start_index += 1;
                        input_vector.push(*time);
                        //println!("increase length");

                        // TODO: check that N values are pushed

                        for i in 0..borrow.len() {
                            let frontier = borrow[i].borrow().frontier();
                            //println!("frontier={:?}", frontier);
                            if frontier.len() == 0 {
                                //println!("$$$$$$$$$$$$$$$$EMPTY FRONTIER$$$$$$$$$$$$$$");
                                current_length += 1;
                                //data_start_index += 1;
                                progress_start_index += 1;
                                input_vector.push(0);
                            } else {
                                for val in frontier.iter() {
                                    current_length += 1;
                                    //data_start_index += 1;
                                    progress_start_index += 1;
                                    input_vector.push((*val << 1) | 1u64);
                                    //println!("increase length by frontier");
                                }
                            }
                        }
                        for i in 0..ghost_indexes.len() {
                            current_length += 4;
                            //data_start_index += 4;
                            input_vector.push(0);
                            input_vector.push(0);
                            input_vector.push(0);
                            input_vector.push(0);
                            //println!("increase length by progress");
                        }
                        for i in current_length..info_length {
                            current_length += 1;
                            input_vector.push(0);
                        }

                        if vector.len() == 0 {
                            current_length += 1;
                            input_vector.push(0 as u64);
                        } else {
                            for val in vector.iter() {
                                current_length += 1;
                                input_vector.push(((*val << 1) | 1u64) as u64);
                            }
                        }

                        for i in current_length..max_length {
                            input_vector.push(0);
                        }

                        println!("PRINT INPUT VECTOR TO FPGA");
                        for elem in input_vector.iter() {
                            print!(" {}", elem);
                        }

                        println!();
                        //send_input(hc, input_vector.as_mut_ptr());
                        let mut output = vec![0;32];

                        if send_input_and_check(hc, input_vector.as_mut_ptr(), output.as_mut_ptr()) {

                            got_output = true;

                            println!("PRINT OUTPUT VECTOR FROM FPGA");
                            for elem in output.iter() {
                                print!(" {}", elem);
                            }
                            println!();


                            for i in data_start_index..max_length {
                                let val = output[i] as u64;
                                let shifted_val = val >> 1;
                                //println!("shifted val = {}", shifted_val);
                                if val != 0 {
                                    vector2.push(shifted_val);
                                }
                            }
                            //vector2.push(1);
                            for (i, j) in ghost_indexes.iter() {
                                //println!("consumed = {}", output[progress_start_index + 4*i]);
                                //println!("internal time = {}", output[progress_start_index + 4*i + 2] >> 1);
                                //println!("internal update {}", output[progress_start_index + 4*i + 3] as i64);
                                //println!("produced = {}", output[progress_start_index + 4*i + 1]);

                                consumed.insert(*j, output[progress_start_index + 4 * i] as i64);
                                internals.insert(*j, ((output[progress_start_index + 4 * i + 2] >> 1 as u64, output[progress_start_index + 4 * i + 3] as i64)));
                                produced.insert(*j, (output[progress_start_index + 4 * i + 1]) as i64);
                            }
                        } else {
                          println!("Not enough time to get output from FPGA");

                        }
                        println!("After check output");

                    }


                    if got_output {
                    // TODO: we leave them empty for now if there is no data
                        output_wrapper.session(time).give_vec(&mut vector2);

                        for (i, j) in ghost_indexes.iter() {

                            let mut cb = ChangeBatch::new_from(time.clone(), *consumed.get(j).unwrap());
                            let mut cb1 = ChangeBatch::new_from(time.clone(), *produced.get(j).unwrap());
                            let mut cb2 = ChangeBatch::new_from(internals.get(j).unwrap().0 as u64, internals.get(j).unwrap().1 as i64);
                            cb.drain_into(&mut progress.wrapper_consumeds.get_mut(j).unwrap()[0]);
                            cb1.drain_into(&mut progress.wrapper_produceds.get_mut(j).unwrap()[0]);
                            cb2.drain_into(&mut progress.wrapper_internals.get_mut(j).unwrap()[0]);
                        }
                    }
                }

                if !has_data {
                    //println!("no data");
                    let mut produced = HashMap::new();
                    let mut consumed = HashMap::new();
                    let mut internals = HashMap::new();
                    let mut info_length = 24;//2 + ghost_indexes.len() + 4 * ghost_indexes.len();
                    let mut all_length = 4;//(((info_length + vector.len()) / 8) + 1) as i64;
                    let mut current_length = 0;
                    let mut max_length = 32;
                    let mut data_start_index = 24;
                    let mut progress_start_index = 0;

                    unsafe {
                        current_length += 1;
                        //data_start_index += 1;
                        progress_start_index += 1;
                        input_vector.push(all_length as u64);
                        //println!("increase length");

                        current_length += 1;
                        //data_start_index += 1;
                        progress_start_index += 1;
                        input_vector.push(0);
                        //println!("increase length");

                        // TODO: check that N values are pushed

                        for i in 0..borrow.len() {
                            let frontier = borrow[i].borrow().frontier();
                            //println!("frontier={:?}", frontier);
                            if frontier.len() == 0 {
                                //println!("$$$$$$$$$$$$$$$$EMPTY FRONTIER$$$$$$$$$$$$$$");
                                current_length += 1;
                                //data_start_index += 1;
                                progress_start_index += 1;
                                input_vector.push(0);
                            } else {
                                for val in frontier.iter() {
                                    current_length += 1;
                                    //data_start_index += 1;
                                    progress_start_index += 1;
                                    input_vector.push((*val << 1) | 1u64);
                                    //println!("increase length by frontier");
                                }
                            }
                        }
                        for i in 0..ghost_indexes.len() {
                            current_length += 4;
                            //data_start_index += 4;
                            input_vector.push(0);
                            input_vector.push(0);
                            input_vector.push(0);
                            input_vector.push(0);
                            //println!("increase length by progress");
                        }
                        //current_length += 1;
                        //input_vector.push( 0 as u64);


                        for i in current_length..max_length {
                            input_vector.push(0);
                        }

                        println!("no data: PRINT INPUT VECTOR TO FPGA");
                        for elem in input_vector.iter() {
                        	print!(" {}", elem);
                        }
                        println!();
                        //send_input(hc, input_vector.as_mut_ptr());

                        let mut output = vec![0;32];
                        if send_input_and_check(hc, input_vector.as_mut_ptr(), output.as_mut_ptr()) {

                            println!("no data: PRINT OUTPUT VECTOR TO FPGA");
                            for elem in output.iter() {
                                print!(" {}", elem);
                            }
                            println!();

                            for i in data_start_index..max_length {
                                let val = output[i] as u64;
                                let shifted_val = val >> 1;
                                //println!("shifted val = {}", shifted_val);
                                if val != 0 {
                                    vector2.push(shifted_val);
                                }
                            }
                            //vector2.push(1);
                            for (i, j) in ghost_indexes.iter() {
                                //println!("consumed = {}", output[progress_start_index + 4*i]);
                                //println!("internal time = {}", output[progress_start_index + 4*i + 2] >> 1);
                                //println!("internal update {}", output[progress_start_index + 4*i + 3] as i64);
                                //println!("produced = {}", output[progress_start_index + 4*i + 1]);

                                consumed.insert(*j, output[progress_start_index + 4 * i] as i64);
                                internals.insert(*j, ((output[progress_start_index + 4 * i + 2] >> 1 as u64, output[progress_start_index + 4 * i + 3] as i64)));
                                produced.insert(*j, (output[progress_start_index + 4 * i + 1]) as i64);
                            }
                        } else {
                            println!("no data: Not enough time to get output from FPGA");
                        }

                        println!("no data: after check output");

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

                output_wrapper.cease();

                let duration = start1.elapsed();

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
        ghost_operators.push(builder_filter.index());
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
            ghost_indexes: ghost_indexes2,
        };

        self.scope().add_operator_with_indices(Box::new(operator), builder_wrapper.index(), builder_wrapper.global());


        // we also need to create a map from ghost to wrapper

        self.scope().add_fpga_operator(builder_wrapper.index(), ghost_operators, ghost_edges);


        return stream_wrapper;
    }
}
