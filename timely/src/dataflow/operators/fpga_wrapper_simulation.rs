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
    //hMem: * mut i64
}

unsafe impl Send for HardwareCommon {}

unsafe impl Sync for HardwareCommon {}


/// Wrapper operator to store ghost operators
struct FpgaOperatorSimulation<T, L>
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

impl<T, L> Schedule for FpgaOperatorSimulation<T, L>
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

impl<T, L> Operate<T> for FpgaOperatorSimulation<T, L>
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
struct FakeOperatorSimulation<T, L>
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

impl<T, L> Schedule for FakeOperatorSimulation<T, L>
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

impl<T, L> Operate<T> for FakeOperatorSimulation<T, L>
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
pub trait FpgaWrapperSimulation<S: Scope/*, D: Data*/> {
    /// Wrapper function
    fn fpga_wrapper_simulation(&self, hc: *const HardwareCommon) -> Stream<S, u64>;
}

// return value should be the value of the last operator

impl<S: Scope<Timestamp=u64>> FpgaWrapperSimulation<S> for Stream<S, u64> {
    fn fpga_wrapper_simulation(&self, hc: *const HardwareCommon) -> Stream<S, u64> {

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

        let operator_filter = FakeOperatorSimulation {
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

        let operator_map = FakeOperatorSimulation {
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

        let operator_aggregate = FakeOperatorSimulation {
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

                let mut consumed = HashMap::new();
                let mut produced = HashMap::new();
                let mut internals = HashMap::new();
                let mut info_length = 24;//2 + ghost_indexes.len() + 4 * ghost_indexes.len();
                let mut all_length = 4;//(((info_length + vector.len()) / 8) + 1) as i64;
                let mut current_length = 0;
                let mut max_length = 32;
                let mut data_start_index = 24;
                let mut progress_start_index = 0;

                current_length += 1;
                //data_start_index += 1;
                progress_start_index += 1;
                input_vector.push(all_length as u64);
                //println!("increase length");

                /* add frontiers */
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


                for i in current_length..info_length {
                    current_length += 1;
                    input_vector.push(0);
                }

                let mut times = Vec::new();


                while let Some(message) = input_wrapper.next() {
                    has_data = true;
                    //println!("INSIDE DATA PROCESSING");
                    let (time, data) = match message.as_ref_or_mut() {
                        RefOrMut::Ref(reference) => (&reference.time, RefOrMut::Ref(&reference.data)),
                        RefOrMut::Mut(reference) => (&reference.time, RefOrMut::Mut(&mut reference.data)),
                    };
                    times.push(time.clone());
                    data.swap(&mut vector);
                    // I should call my fpga function here with vector as an input


                    current_length += 1;
                    //data_start_index += 1;
                    progress_start_index += 1;
                    input_vector.push(*time);
                    //println!("increase length");

                    current_length += 1;
                    //data_start_index += 1;
                    progress_start_index += 1;
                    input_vector.push(vector.len() as u64);
                    //println!("increase length");


                    if vector.len() == 0 {
                        current_length += 1;
                        input_vector.push(0 as u64);
                    } else {
                        for val in vector.iter() {
                            current_length += 1;
                            input_vector.push(((*val << 1) | 1u64) as u64);
                        }
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

                /* so far we put all data for all times*/

                let mut output = Vec::new();
                output.push(input_vector[0]); // all length
                output.push(input_vector[1]); // time
                output.push(input_vector[2]); // frontier 1
                output.push(input_vector[3]); // frontier 2
                //output.push(input_vector[4]); // frontier 3
                output.push(1); // consumed 1
                output.push(0); // internals 1
                output.push(1); // produceds 1
                output.push(1); // consumed 2
                output.push(0); // internals 2
                output.push(1); // produceds 2
                //output.push(1); // consumed 3
                //output.push(0); // internals 3
                //output.push(1); // produceds 3
                output.push(3); // data
                output.push(0);
                output.push(0);
                output.push(0);
                output.push(0);output.push(0); output.push(0);
                output.push(0); output.push(0); output.push(0);
                output.push(0); output.push(0); output.push(0);

                // TODO: insert a delay to simulatу checking output
                // TODO: introduce a new fгnction which checks if there is a result
                println!("PRINT OUTPUT VECTOR FROM FPGA");
                for (i, elem) in output.iter().enumerate() {
                    print!(" {}", elem);
                }
                println!();

                let mut i = data_start_index;
                while (i != max_length) {
                    let sub_length = output[i+1] as usize;
                    let mut sub_vector = Vec::new();

                    for j in i + 1 .. i + sub_length + 1 {
                        let val = output[i] as u64;
                        let shifted_val = val >> 1;
                        if (val != 0) {
                            sub_vector.push(output[j]);
                        }
                    }
                    vector2.push((output[i] as S::Timestamp, sub_vector));
                }


                // |t1|c1|p1|it1|iu1|c2|p2|it2|iu2|...|t2|c1|p1|it1|iu1|c2|p2|it2|iu2
                // progress_start_index
                //  ^
                //  |
                for (t, time) in times.iter().enumerate() {
                    consumed.insert(time, Vec::new());
                    produced.insert(time, Vec::new());
                    internals.insert(time, Vec::new());
                    for (g, index) in ghost_indexes.iter().enumerate() {

                        consumed.get_mut(time).unwrap().push(output[progress_start_index + 13 * t + 4 * g + 1] as i64);
                        produced.get_mut( time).unwrap().push( output[progress_start_index + 13 * t + 4 * g + 2] as i64);
                        internals.get_mut(time).unwrap().push ((
                            output[progress_start_index + 13 * t + 4 * g + 3] >> 1 as u64,
                            output[progress_start_index + 13 * t + 4 * g + 4] as i64));
                    }
                }



                for (time, mut data) in vector2.clone() {
                    output_wrapper.session( &time).give_vec(&mut data);
                }
                // for now per one time only one change in internals

                for ( time, data) in consumed.clone() {
                    for (g, element) in data.iter().enumerate() {
                        let mut con = ChangeBatch::new_from(time.clone(), *element);
                        con.drain_into(&mut progress.wrapper_consumeds.get_mut(&g).unwrap()[0]);
                    }
                }
                for ( time, data) in produced.clone() {
                    for (g, element) in data.iter().enumerate() {
                        let mut pro = ChangeBatch::new_from(time.clone(), *element);
                        pro.drain_into(&mut progress.wrapper_produceds.get_mut(&g).unwrap()[0]);
                    }
                }
                for (time, data) in internals.clone() {
                    for (g, element) in data.iter().enumerate() {
                        let mut int = ChangeBatch::new_from(element.0 as u64, element.1 as i64);
                        int.drain_into(&mut progress.wrapper_internals.get_mut(&g).unwrap()[0]);
                    }
                }

                if !has_data {
                    for i in current_length..max_length {
                        input_vector.push(0);
                    }

                    println!("PRINT INPUT VECTOR TO FPGA");
                    for elem in input_vector.iter() {
                        print!(" {}", elem);
                    }
                    println!();


                    let mut output = Vec::new();
                    output.push(input_vector[0]); // all length
                    output.push(input_vector[1]); // time
                    output.push(input_vector[2]); // frontier 1
                    output.push(input_vector[3]); // frontier 2
                    //output.push(input_vector[4]); // frontier 3
                    output.push(1); // consumed 1
                    output.push(0); // internals 1
                    output.push(1); // produceds 1
                    output.push(1); // consumed 2
                    output.push(0); // internals 2
                    output.push(1); // produceds 2
                    //output.push(1); // consumed 3
                    //output.push(0); // internals 3
                    //output.push(1); // produceds 3
                    output.push(3); // data
                    output.push(0);
                    output.push(0);
                    output.push(0);
                    output.push(0);output.push(0); output.push(0);
                    output.push(0); output.push(0); output.push(0);
                    output.push(0); output.push(0); output.push(0);


                    println!("PRINT OUTPUT VECTOR FROM FPGA");
                                for (i, elem) in output.iter().enumerate() {
                                    print!(" {}", elem);
                                }
                    println!();

                    let mut i = data_start_index;
                    while (i != max_length) {
                        let sub_length = output[i+1] as usize;
                        let mut sub_vector = Vec::new();

                        for j in i + 1 .. i + sub_length + 1 {
                            let val = output[i] as u64;
                            let shifted_val = val >> 1;
                            if (val != 0) {
                                sub_vector.push(output[j]);
                            }
                        }
                        vector2.push((output[i], sub_vector));
                    }

                    // |t1|c1|p1|it1|iu1|c2|p2|it2|iu2|...|t2|c1|p1|it1|iu1|c2|p2|it2|iu2
                    // progress_start_index
                    //  ^
                    //  |
                    for (t, time) in times.iter().enumerate() {
                        consumed.insert(time, Vec::new());
                        produced.insert(time, Vec::new());
                        internals.insert(time, Vec::new());
                        for (g, index) in ghost_indexes.iter().enumerate() {

                            consumed.get_mut(time).unwrap().push(output[progress_start_index + 13 * t + 4 * g + 1] as i64);
                            produced.get_mut( time).unwrap().push( output[progress_start_index + 13 * t + 4 * g + 2] as i64);
                            internals.get_mut(time).unwrap().push ((
                                output[progress_start_index + 13 * t + 4 * g + 3] >> 1 as u64,
                                output[progress_start_index + 13 * t + 4 * g + 4] as i64));
                        }
                    }


                    /*for ( time, data) in consumed {
                        for (g, element) in data.iter().enumerate() {
                            let mut con = ChangeBatch::new_from(time.clone(), *element);
                            con.drain_into(&mut progress.wrapper_consumeds.get_mut(&g).unwrap()[0]);
                        }
                    }
                    for ( time, data) in produced {
                        for (g, element) in data.iter().enumerate() {
                            let mut pro = ChangeBatch::new_from(time.clone(), *element);
                            pro.drain_into(&mut progress.wrapper_produceds.get_mut(&g).unwrap()[0]);
                        }
                    }
                    for (time, data) in internals {
                        for (g, element) in data.iter().enumerate() {
                            let mut int = ChangeBatch::new_from(element.0 as u64, element.1 as i64);
                            int.drain_into(&mut progress.wrapper_internals.get_mut(&g).unwrap()[0]);
                        }
                    }*/
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
        let operator = FpgaOperatorSimulation {
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
