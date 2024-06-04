//! Funtionality to run operators on FPGA
pub extern crate libc;

use crate::dataflow::channels::pact::Pipeline;
use crate::dataflow::channels::pullers::Counter as PullCounter;
use crate::dataflow::channels::pushers::buffer::Buffer as PushBuffer;
use crate::dataflow::channels::pushers::Counter as PushCounter;
use crate::dataflow::operators::generic::builder_raw::OperatorBuilder;
use crate::dataflow::operators::generic::builder_raw::OperatorShape;
use crate::dataflow::{Scope, Stream};
use crate::progress::{operate::SharedProgress, Antichain, ChangeBatch, Operate, Timestamp};
use crate::scheduling::{Activations, Schedule};


use crate::dataflow::operators::fakeoperator::FakeOperator;
use crate::dataflow::operators::fakeoperator::FpgaOperator;

use crate::progress::frontier::MutableAntichain;
use std::cell::RefCell;
use std::convert::TryInto;
use std::rc::Rc;
use std::time::Instant;
use std::borrow::Borrow;

use std::collections::HashMap;
use std::ffi::c_void;

use std::simd::{i64x2, i64x4, u64x2, u64x4};
//#[rustversion::nightly]
use std::simd::Simd;

// Various parameters
const CACHE_LINE_SIZE: i64 = 16;
const MAX_CAPACITY: usize = 8192;

/*const BATCH_SIZE: usize = BATCH_LINES * CACHE_LINE_SIZE;
const NUMBER_OF_INPUTS: usize = BATCH_SIZE; // make sure to sync with caller (e.g. `hello_fpga.rs`)
const NUMBER_OF_FILTER_OPERATORS: usize = 1;
const NUMBER_OF_MAP_OPERATORS: usize = 0;
const OPERATOR_COUNT: usize = NUMBER_OF_FILTER_OPERATORS + NUMBER_OF_MAP_OPERATORS;
const FRONTIER_LENGTH: usize = CACHE_LINE_SIZE;
const MAX_LENGTH_IN: usize = FRONTIER_LENGTH + NUMBER_OF_INPUTS;
const DATA_LENGTH: usize = BATCH_SIZE;
const PROGRESS_START_INDEX: usize = DATA_LENGTH;
const PROGRESS_OUTPUT: usize = CACHE_LINE_SIZE;
const MAX_LENGTH_OUT: usize = BATCH_SIZE + CACHE_LINE_SIZE;*/

//input_arr: [u64; MAX_LENGTH_IN]
//output_arr: [u64; MAX_LENGTH_OUT]

static mut GLOBAL_COUNTER: i32 = 0;

/// Fence needed for syncing memory operations
#[cfg(target_arch = "aarch64")]
#[inline]
fn dmb() {
    core::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
}

/// Fence needed for syncing memory operations
#[cfg(target_arch = "x86_64")]
#[inline]
fn dmb() {
    core::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
}

#[repr(C)]
/// Data structure to store FPGA related data
pub struct HardwareCommon {
    /// TADA
    pub area: * mut c_void
}


unsafe impl Send for HardwareCommon{}
unsafe impl Sync for HardwareCommon{}

/// Wrapper to run on FPGA
pub trait FpgaWrapperPCI<S: Scope> {
    /// Wrapper function
    fn fpga_wrapper_pci(&self, num_data: i64, num_operators: i64, hc: *const HardwareCommon) -> Stream<S, u64>;
}

// return value should be the value of the last operator

impl<S: Scope<Timestamp = u64>> FpgaWrapperPCI<S> for Stream<S, u64> {
    fn fpga_wrapper_pci(&self, num_data: i64, num_operators: i64, hc: *const HardwareCommon) -> Stream<S, u64> {
        // this should correspond to the way the data will be read on the fpga
        let mut ghost_indexes = Vec::new();
        let mut ghost_indexes2 = Vec::new();
        // TODO: should get rid of ghost indexes
        let mut current_index = 0;

        let mut frontier_length = 24;//(num_operators / CACHE_LINE_SIZE) + CACHE_LINE_SIZE;
        let mut progress_length = 16;//((num_operators * 4) / CACHE_LINE_SIZE) + CACHE_LINE_SIZE;

        let max_length_in = num_data as usize + frontier_length as usize;
        let max_length_out = num_data as usize + progress_length as usize;
        let progress_start_index = num_data as usize;


        let mut vec_builder_filter = vec![];
        for i in 0..num_operators {
            // CREATE FILTER GHOST OPERATOR 1
            let mut builder_filter =
                OperatorBuilder::new(format!("Filter{}", i + 1).to_owned(), self.scope()); // scope comes from stream
            builder_filter.set_notify(false);
            builder_filter.set_shape(1, 1);

            let operator_logic_filter = move |_progress: &mut SharedProgress<S::Timestamp>| false;

            let operator_filter1 = FakeOperator {
                shape: builder_filter.shape().clone(),
                address: builder_filter.address().clone(),
                activations: self.scope().activations().clone(),
                logic: operator_logic_filter,
                shared_progress: Rc::new(RefCell::new(SharedProgress::new(1, 1))),
                summary: builder_filter.summary().to_vec(),
            };

            self.scope().add_operator_with_indices_no_path(
                Box::new(operator_filter1),
                builder_filter.index(),
                builder_filter.global(),
            );
            ghost_indexes.push((current_index, builder_filter.index()));
            ghost_indexes2.push((current_index, builder_filter.index()));
            current_index += 1;

            vec_builder_filter.push(builder_filter);
        }

        // create wrapper operator

        let mut builder_wrapper = OperatorBuilder::new("Wrapper".to_owned(), self.scope()); // scope comes from stream
        let mut input_wrapper = PullCounter::new(builder_wrapper.new_input(self, Pipeline)); // builder.new_input -> creates new Input and new input connection in builder_raw.rs
        let (tee_wrapper, stream_wrapper) = builder_wrapper.new_output();
        // this stream is returned every time, Rust will probably complain.
        // create new_output_connection function without returning the stream?
        let mut output_wrapper = PushBuffer::new(PushCounter::new(tee_wrapper));

        let frontier = Rc::new(RefCell::new(vec![
            MutableAntichain::new();
            ghost_indexes.len()
        ]));
        let mut started = false;

        let mut vector = Vec::with_capacity(MAX_CAPACITY);
        let mut vector2 = Vec::with_capacity(MAX_CAPACITY);



        let raw_logic = move |progress: &mut SharedProgress<S::Timestamp>| {

            //println! ("Start logic");

            //let epoch_start = Instant::now();

            let mut data_length: i64 = 16;

            let mut borrow = frontier.borrow_mut();

            for (i, j) in ghost_indexes.iter() {
                borrow[*i].update_iter(progress.wrapper_frontiers.get_mut(j).unwrap()[0].drain());
            }

            if !started {
                // discard initial capability.
                for (_i, j) in ghost_indexes.iter() {
                    progress.wrapper_internals.get_mut(j).unwrap()[0]
                        .update(S::Timestamp::minimum(), -1);
                    started = true;
                }
            }

            // invoke supplied logic
            use crate::communication::message::RefOrMut;

            let mut has_data = false;

            //let epoch_start = Instant::now();

            while let Some(message) = input_wrapper.next() {

                has_data = true;
                let (time, data) = match message.as_ref_or_mut() {
                    RefOrMut::Ref(reference) => (&reference.time, RefOrMut::Ref(&reference.data)),
                    RefOrMut::Mut(reference) => {
                        (&reference.time, RefOrMut::Mut(&mut reference.data))
                    }
                };
                data.swap(&mut vector);

                //println!("Has data");

                let mut current_length = 0;
                let area = unsafe { (*hc).area } as *mut u64;
                let mut v1: Vec<u64x2> = Vec::new();
                let mut v0: Vec<u64x2> = Vec::new();


                for i in (0..29).step_by(2) {
                    let frontier1 = borrow[i].borrow().frontier();
                    let frontier2 = borrow[i+1].borrow().frontier();

                    let x =  u64x2::from_array([(frontier1[0] << 1) | 1u64, (frontier2[0] << 1) | 1u64]);
                    v0.push(x);
                    current_length += 2;
                }



                let frontier_last = borrow[30].borrow().frontier();
                let x =  u64x2::from_array([(frontier_last[0] << 1) | 1u64, 0]);
                v0.push(x);

                //println!("Current length = {}", current_length);

                for i in (0..16).step_by(2) {
                    let x = u64x2::from_array([(vector[i] << 1) | 1u64, (vector[i+1] << 1) | 1u64]);
                    v0.push(x);
                }
//--------------------------------------------------------------------------------------------- print the output data
                /*println!("OUTPUT DATA FROM TIMELY");
                println!("Length of frontier vector {}", v0.len());
                for val in &v0 {
                    println!("{} {}", val[0], val[1]);
                }
                println!();*/

                /*println!("Length of data vector {}", v1.len());
                for val in &v1 {
                    println!("{} {}", val[0], val[1]);
                }
                println!();*/
//--------------------------------------------------------------------------------------------- print the output data

                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe{*(area.offset(0 as isize) as *mut u64x2) = v0[0]};
                    unsafe{*(area.offset(2 as isize) as *mut u64x2) = v0[1]};
                    unsafe{*(area.offset(4 as isize) as *mut u64x2) = v0[2]};
                    unsafe{*(area.offset(6 as isize) as *mut u64x2) = v0[3]};
                    unsafe{*(area.offset(8 as isize) as *mut u64x2) = v0[4]};
                    unsafe{*(area.offset(10 as isize) as *mut u64x2) = v0[5]};
                    unsafe{*(area.offset(12 as isize) as *mut u64x2) = v0[6]};
                    unsafe{*(area.offset(14 as isize) as *mut u64x2) = v0[7]};
                    unsafe{*(area.offset(16 as isize) as *mut u64x2) = v0[8]};
                    unsafe{*(area.offset(18 as isize) as *mut u64x2) = v0[9]};
                    unsafe{*(area.offset(20 as isize) as *mut u64x2) = v0[10]};
                    unsafe{*(area.offset(22 as isize) as *mut u64x2) = v0[11]};
                    unsafe{*(area.offset(24 as isize) as *mut u64x2) = v0[12]};
                    unsafe{*(area.offset(26 as isize) as *mut u64x2) = v0[13]};
                    unsafe{*(area.offset(28 as isize) as *mut u64x2) = v0[14]};
                    unsafe{*(area.offset(30 as isize) as *mut u64x2) = v0[15]};
                    unsafe{*(area.offset(32 as isize) as *mut u64x2) = v0[16]};
                    unsafe{*(area.offset(34 as isize) as *mut u64x2) = v0[17]};
                    unsafe{*(area.offset(36 as isize) as *mut u64x2) = v0[18]};
                    unsafe{*(area.offset(38 as isize) as *mut u64x2) = v0[19]};
                    unsafe{*(area.offset(40 as isize) as *mut u64x2) = v0[20]};
                    unsafe{*(area.offset(42 as isize) as *mut u64x2) = v0[21]};
                    unsafe{*(area.offset(44 as isize) as *mut u64x2) = v0[22]};
                    unsafe{*(area.offset(46 as isize) as *mut u64x2) = v0[23]};

                    dmb();
                }
                let mut pc: i64x2 = i64x2::from_array([0 , 0]);
                let mut it: i64x2 = i64x2::from_array([0 , 0]);
                let mut data: u64x2 = u64x2::from_array([0 , 0]);

                //println!("INPUT DATA TO TIMELY (non filtered)");
                //dmb();
                #[cfg(not(feature = "no-fpga"))] {
                    for i in (0..data_length).step_by(2) {
                        //dmb();
                        unsafe { data = *(area.offset(i as isize) as *mut u64x2); }
                        // all the writes can be done asynchronously
                        // we are getting two numbers here
                        // the offset for progress would be 18
                        //println!("{} {}", data[0], data[1]);
                        dmb();
                        let shifted_val1 = data[0] >> 1;
                        let shifted_val2 = data[1] >> 1;
                        if data[0] != 0 {
                            vector2.push(shifted_val1);
                        }
                        if data[1] != 0 {
                            vector2.push(shifted_val2);
                        }
                    }
                }
                //dmb();

                /*println!("INPUT DATA TO TIMELY");
                println!("Length of frontier vector {}", v0.len());
                for val in &vector2 {
                    print!("{} ", val);
                }
                println!();
                */


                #[cfg(feature = "no-fpga")] {
                    for i in 0..16 {
                        vector2.push(0);
                    }
                }
                output_wrapper.session(time).give_vec(&mut vector2);

                let mut k = 0;
                let mut i = 0 as usize;
                let mut j = 0;
                let mut cb = ChangeBatch::new_from(0, 0);
                let mut cb1 = ChangeBatch::new_from(0, 0);
                let mut cb2 = ChangeBatch::new_from(0, 0);
                let mut counter_offset = 0;

                let time_1 = time.clone();

// ---------------------------------------------------------------------------------- get the data
                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe { pc = *(area.offset(16 + 0 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(16 + 2 as isize) as *mut i64x2); }
                    dmb();
                    //println!("{} {}", pc[0], pc[1]);
                    //println!("{} {}", it[0], it[1]);
                }
// ---------------------------------------------------------------------------------- got data
                #[cfg(feature = "no-fpga")] {
                    pc = i64x2::from_array([16, 16]);
                    it = i64x2::from_array([0, 0]);
                }
// ----------------------------------------------------------------------------------- for the debug mode

                cb = ChangeBatch::new_from(time_1, pc[0] as i64 );
                cb1 = ChangeBatch::new_from(time_1, pc[1] as i64 );
                cb2 = ChangeBatch::new_from(
                    it[0] as u64,
                    it[1] as i64,
                );
                j = ghost_indexes[0].1 as usize;
                cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe { pc = *(area.offset(16 + 4 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(16 + 6 as isize) as *mut i64x2); }
                    dmb();
                    //println!("{} {}", pc[0], pc[1]);
                    //println!("{} {}", it[0], it[1]);
                }
// ---------------------------------------------------------------------------------- got data
                #[cfg(feature = "no-fpga")] {
                    pc = i64x2::from_array([16, 16]);
                    it = i64x2::from_array([0, 0]);
                }
// ----------------------------------------------------------------------------------- for the debug mode
                cb = ChangeBatch::new_from(time_1, pc[0] as i64 );
                cb1 = ChangeBatch::new_from(time_1, pc[1] as i64 );
                cb2 = ChangeBatch::new_from(
                    it[0] as u64,
                    it[1] as i64,
                );
                j = ghost_indexes[1].1 as usize;
                cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe { pc = *(area.offset(16 + 8 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(16 + 10 as isize) as *mut i64x2); }
                    dmb();
                    //println!("{} {}", pc[0], pc[1]);
                    //println!("{} {}", it[0], it[1]);
                }
// ---------------------------------------------------------------------------------- got data

                #[cfg(feature = "no-fpga")] {
                    pc = i64x2::from_array([16, 16]);
                    it = i64x2::from_array([0, 0]);
                }
// ----------------------------------------------------------------------------------- for the debug mode
                cb = ChangeBatch::new_from(time_1, pc[0] as i64 );
                cb1 = ChangeBatch::new_from(time_1, pc[1] as i64 );
                cb2 = ChangeBatch::new_from(
                    it[0] as u64,
                    it[1] as i64,
                );

                j = ghost_indexes[2].1 as usize;
                cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe { pc = *(area.offset(16 + 12 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(16 + 14 as isize) as *mut i64x2); }
                    dmb();
                    //println!("{} {}", pc[0], pc[1]);
                    //println!("{} {}", it[0], it[1]);
                }
// ---------------------------------------------------------------------------------- got data
                #[cfg(feature = "no-fpga")] {
                    pc = i64x2::from_array([16, 16]);
                    it = i64x2::from_array([0, 0]);
                }
// ----------------------------------------------------------------------------------- for the debug mode
                cb = ChangeBatch::new_from(time_1, pc[0] as i64 );
                cb1 = ChangeBatch::new_from(time_1, pc[1] as i64 );
                cb2 = ChangeBatch::new_from(
                    it[0] as u64,
                    it[1] as i64,
                );
                j = ghost_indexes[3].1 as usize;
                cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
                //println!("DONE 4");

// ---------------------------------------------------------------------------------- get the data
                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe{pc = *(area.offset(16 + 16 as isize) as *mut i64x2);}
                    dmb();
                    unsafe{it = *(area.offset(16 + 18 as isize) as *mut i64x2);}
                    dmb();
                    //println!("{} {}", pc[0], pc[1]);
                    //println!("{} {}", it[0], it[1]);
                }
// ---------------------------------------------------------------------------------- got data
                #[cfg(feature = "no-fpga")] {
                    pc = i64x2::from_array([16, 16]);
                    it = i64x2::from_array([0, 0]);
                }
// ----------------------------------------------------------------------------------- for the debug mode
                cb = ChangeBatch::new_from(time_1, pc[0] as i64 );
                cb1 = ChangeBatch::new_from(time_1, pc[1] as i64 );
                cb2 = ChangeBatch::new_from(
                    it[0] as u64,
                    it[1] as i64,
                );
                j = ghost_indexes[4].1 as usize;
                cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe { pc = *(area.offset(16 + 20 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(16 + 22 as isize) as *mut i64x2); }
                    dmb();
                    //println!("{} {}", pc[0], pc[1]);
                    //println!("{} {}", it[0], it[1]);
                }
// ---------------------------------------------------------------------------------- got data
                #[cfg(feature = "no-fpga")] {
                    pc = i64x2::from_array([16, 16]);
                    it = i64x2::from_array([0, 0]);
                }
// ----------------------------------------------------------------------------------- for the debug mode

                cb = ChangeBatch::new_from(time_1, pc[0] as i64 );
                cb1 = ChangeBatch::new_from(time_1, pc[1] as i64 );
                cb2 = ChangeBatch::new_from(
                    it[0] as u64,
                    it[1] as i64,
                );
                j = ghost_indexes[5].1 as usize;
                cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe { pc = *(area.offset(16 + 24 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(16 + 26 as isize) as *mut i64x2); }
                    dmb();
                    //println!("{} {}", pc[0], pc[1]);
                    //println!("{} {}", it[0], it[1]);
                }
// ---------------------------------------------------------------------------------- get the data
                #[cfg(feature = "no-fpga")] {
                    pc = i64x2::from_array([16, 16]);
                    it = i64x2::from_array([0, 0]);
                }
// ----------------------------------------------------------------------------------- for the debug mode

                cb = ChangeBatch::new_from(time_1, pc[0] as i64 );
                cb1 = ChangeBatch::new_from(time_1, pc[1] as i64 );
                cb2 = ChangeBatch::new_from(
                    it[0] as u64,
                    it[1] as i64,
                );

                j = ghost_indexes[6].1 as usize;
                cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);


// ---------------------------------------------------------------------------------- get the data
                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe { pc = *(area.offset(16 + 28 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(16 + 30 as isize) as *mut i64x2); }
                    dmb();
                    //println!("{} {}", pc[0], pc[1]);
                    //println!("{} {}", it[0], it[1]);
                }

// ---------------------------------------------------------------------------------- get the data
                #[cfg(feature = "no-fpga")] {
                    pc = i64x2::from_array([16, 16]);
                    it = i64x2::from_array([0, 0]);
                }
// ----------------------------------------------------------------------------------- for the debug mode
                cb = ChangeBatch::new_from(time_1, pc[0] as i64 );
                cb1 = ChangeBatch::new_from(time_1, pc[1] as i64 );
                cb2 = ChangeBatch::new_from(
                    it[0] as u64,
                    it[1] as i64,
                );
                j = ghost_indexes[7].1 as usize;
                cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
                //println!("DONE 5");

// ---------------------------------------------------------------------------------- get the data
                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe { pc = *(area.offset(16 + 32 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(16 + 34 as isize) as *mut i64x2); }
                    dmb();
                    //println!("{} {}", pc[0], pc[1]);
                    //println!("{} {}", it[0], it[1]);
                }
// ---------------------------------------------------------------------------------- get the data
                #[cfg(feature = "no-fpga")] {
                    pc = i64x2::from_array([16, 16]);
                    it = i64x2::from_array([0, 0]);
                }
// ----------------------------------------------------------------------------------- for the debug mode

                cb = ChangeBatch::new_from(time_1, pc[0] as i64 );
                cb1 = ChangeBatch::new_from(time_1, pc[1] as i64 );
                cb2 = ChangeBatch::new_from(
                    it[0] as u64,
                    it[1] as i64,
                );
                j = ghost_indexes[8].1 as usize;
                cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
                i = i + 4;

// ---------------------------------------------------------------------------------- get the data
                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe { pc = *(area.offset(16 + 36 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(16 + 38 as isize) as *mut i64x2); }
                    dmb();
                    //println!("{} {}", pc[0], pc[1]);
                    //println!("{} {}", it[0], it[1]);
                }
// ---------------------------------------------------------------------------------- get the data
                #[cfg(feature = "no-fpga")] {
                    pc = i64x2::from_array([16, 16]);
                    it = i64x2::from_array([0, 0]);
                }
// ----------------------------------------------------------------------------------- for the debug mode

                cb = ChangeBatch::new_from(time_1, pc[0] as i64 );
                cb1 = ChangeBatch::new_from(time_1, pc[1] as i64 );
                cb2 = ChangeBatch::new_from(
                    it[0] as u64,
                    it[1] as i64,
                );
                j = ghost_indexes[9].1 as usize;
                cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe { pc = *(area.offset(16 + 40 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(16 + 42 as isize) as *mut i64x2); }
                    dmb();
                    //println!("{} {}", pc[0], pc[1]);
                    //println!("{} {}", it[0], it[1]);
                }
// ---------------------------------------------------------------------------------- get the data
                #[cfg(feature = "no-fpga")] {
                    pc = i64x2::from_array([16, 16]);
                    it = i64x2::from_array([0, 0]);
                }
// ----------------------------------------------------------------------------------- for the debug mode
                cb = ChangeBatch::new_from(time_1, pc[0] as i64 );
                cb1 = ChangeBatch::new_from(time_1, pc[1] as i64 );
                cb2 = ChangeBatch::new_from(
                    it[0] as u64,
                    it[1] as i64,
                );

                j = ghost_indexes[10].1 as usize;
                cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe { pc = *(area.offset(16 + 44 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(16 + 46 as isize) as *mut i64x2); }
                    dmb();
                    //println!("{} {}", pc[0], pc[1]);
                    //println!("{} {}", it[0], it[1]);
                }
// ---------------------------------------------------------------------------------- get the data
                #[cfg(feature = "no-fpga")] {
                    pc = i64x2::from_array([16, 16]);
                    it = i64x2::from_array([0, 0]);
                }
// ----------------------------------------------------------------------------------- for the debug mode

                cb = ChangeBatch::new_from(time_1, pc[0] as i64 );
                cb1 = ChangeBatch::new_from(time_1, pc[1] as i64 );
                cb2 = ChangeBatch::new_from(
                    it[0] as u64,
                    it[1] as i64,
                );
                j = ghost_indexes[11].1 as usize;
                cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
                //println!("DONE 6");

// ---------------------------------------------------------------------------------- get the data
                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe { pc = *(area.offset(16 + 48 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(16 + 50 as isize) as *mut i64x2); }
                    dmb();
                    //println!("{} {}", pc[0], pc[1]);
                    //println!("{} {}", it[0], it[1]);
                }
// ---------------------------------------------------------------------------------- get the data

                #[cfg(feature = "no-fpga")] {
                    pc = i64x2::from_array([16, 16]);
                    it = i64x2::from_array([0, 0]);
                }
// ----------------------------------------------------------------------------------- for the debug mode
                cb = ChangeBatch::new_from(time_1, pc[0] as i64 );
                cb1 = ChangeBatch::new_from(time_1, pc[1] as i64 );
                cb2 = ChangeBatch::new_from(
                    it[0] as u64,
                    it[1] as i64,
                );
                j = ghost_indexes[12].1 as usize;
                cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
                i = i + 4;

// ---------------------------------------------------------------------------------- get the data
                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe { pc = *(area.offset(16 + 52 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(16 + 54 as isize) as *mut i64x2); }
                    dmb();
                    //println!("{} {}", pc[0], pc[1]);
                    //println!("{} {}", it[0], it[1]);
                }
// ---------------------------------------------------------------------------------- get the data
                #[cfg(feature = "no-fpga")] {
                    pc = i64x2::from_array([16, 16]);
                    it = i64x2::from_array([0, 0]);
                }
// ----------------------------------------------------------------------------------- for the debug mode

                cb = ChangeBatch::new_from(time_1, pc[0] as i64 );
                cb1 = ChangeBatch::new_from(time_1, pc[1] as i64 );
                cb2 = ChangeBatch::new_from(
                    it[0] as u64,
                    it[1] as i64,
                );
                j = ghost_indexes[13].1 as usize;
                cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe { pc = *(area.offset(16 + 56 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(16 + 58 as isize) as *mut i64x2); }
                    dmb();
                    //println!("{} {}", pc[0], pc[1]);
                    //println!("{} {}", it[0], it[1]);
                }
// ---------------------------------------------------------------------------------- get the data
                #[cfg(feature = "no-fpga")] {
                    pc = i64x2::from_array([16, 16]);
                    it = i64x2::from_array([0, 0]);
                }
// ----------------------------------------------------------------------------------- for the debug mode

                cb = ChangeBatch::new_from(time_1, pc[0] as i64 );
                cb1 = ChangeBatch::new_from(time_1, pc[1] as i64 );
                cb2 = ChangeBatch::new_from(
                    it[0] as u64,
                    it[1] as i64,
                );
                j = ghost_indexes[14].1 as usize;
                cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

                // ---------------------------------------------------------------------------------- get the data
                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe { pc = *(area.offset(16 + 60 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(16 + 62 as isize) as *mut i64x2); }
                    dmb();
                    //println!("{} {}", pc[0], pc[1]);
                    //println!("{} {}", it[0], it[1]);
                }
// ---------------------------------------------------------------------------------- get the data
                #[cfg(feature = "no-fpga")] {
                    pc = i64x2::from_array([16, 16]);
                    it = i64x2::from_array([0, 0]);
                }
// ----------------------------------------------------------------------------------- for the debug mode
                cb = ChangeBatch::new_from(time_1, pc[0] as i64 );
                cb1 = ChangeBatch::new_from(time_1, pc[1] as i64 );
                cb2 = ChangeBatch::new_from(
                    it[0] as u64,
                    it[1] as i64,
                );
                j = ghost_indexes[15].1 as usize;
                cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the
                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe { pc = *(area.offset(16 + 64 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(16 + 66 as isize) as *mut i64x2); }
                    dmb();
                    //println!("{} {}", pc[0], pc[1]);
                    //println!("{} {}", it[0], it[1]);
                }
// ---------------------------------------------------------------------------------- get the data
                #[cfg(feature = "no-fpga")] {
                    pc = i64x2::from_array([16, 16]);
                    it = i64x2::from_array([0, 0]);
                }
// ----------------------------------------------------------------------------------- for the debug mode

                cb = ChangeBatch::new_from(time_1, pc[0] as i64 );
                cb1 = ChangeBatch::new_from(time_1, pc[1] as i64 );
                cb2 = ChangeBatch::new_from(
                    it[0] as u64,
                    it[1] as i64,
                );
                j = ghost_indexes[16].1 as usize;
                cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe { pc = *(area.offset(16 + 68 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(16 + 70 as isize) as *mut i64x2); }
                    dmb();
                    //println!("{} {}", pc[0], pc[1]);
                    //println!("{} {}", it[0], it[1]);
                }
// ---------------------------------------------------------------------------------- get the data
                #[cfg(feature = "no-fpga")] {
                    pc = i64x2::from_array([16, 16]);
                    it = i64x2::from_array([0, 0]);
                }
// ----------------------------------------------------------------------------------- for the debug mode

                cb = ChangeBatch::new_from(time_1, pc[0] as i64 );
                cb1 = ChangeBatch::new_from(time_1, pc[1] as i64 );
                cb2 = ChangeBatch::new_from(
                    it[0] as u64,
                    it[1] as i64,
                );
                j = ghost_indexes[17].1 as usize;
                cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe { pc = *(area.offset(16 + 72 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(16 + 74 as isize) as *mut i64x2); }
                    dmb();
                    //println!("{} {}", pc[0], pc[1]);
                    //println!("{} {}", it[0], it[1]);
                }
// ---------------------------------------------------------------------------------- get the data
                #[cfg(feature = "no-fpga")] {
                    pc = i64x2::from_array([16, 16]);
                    it = i64x2::from_array([0, 0]);
                }
// ----------------------------------------------------------------------------------- for the debug mode

                cb = ChangeBatch::new_from(time_1, pc[0] as i64 );
                cb1 = ChangeBatch::new_from(time_1, pc[1] as i64 );
                cb2 = ChangeBatch::new_from(
                    it[0] as u64,
                    it[1] as i64,
                );
                j = ghost_indexes[18].1 as usize;
                cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);


// ---------------------------------------------------------------------------------- get the data
                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe { pc = *(area.offset(16 + 76 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(16 + 78 as isize) as *mut i64x2); }
                    dmb();
                    //println!("{} {}", pc[0], pc[1]);
                    //println!("{} {}", it[0], it[1]);
                }
// ---------------------------------------------------------------------------------- get the data
                #[cfg(feature = "no-fpga")] {
                    pc = i64x2::from_array([16, 16]);
                    it = i64x2::from_array([0, 0])
                }
// ----------------------------------------------------------------------------------- for the debug mode

                cb = ChangeBatch::new_from(time_1, pc[0] as i64 );
                cb1 = ChangeBatch::new_from(time_1, pc[1] as i64 );
                cb2 = ChangeBatch::new_from(
                    it[0] as u64,
                    it[1] as i64,
                );
                j = ghost_indexes[19].1 as usize;
                cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

                // ---------------------------------------------------------------------------------- get the data
                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe { pc = *(area.offset(16 + 80 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(16 + 82 as isize) as *mut i64x2); }
                    dmb();
                    //println!("{} {}", pc[0], pc[1]);
                    //println!("{} {}", it[0], it[1]);
                }
// ---------------------------------------------------------------------------------- get the data
                #[cfg(feature = "no-fpga")] {
                    pc = i64x2::from_array([16, 16]);
                    it = i64x2::from_array([0, 0])
                }
// ----------------------------------------------------------------------------------- for the debug mode

                cb = ChangeBatch::new_from(time_1, pc[0] as i64 );
                cb1 = ChangeBatch::new_from(time_1, pc[1] as i64 );
                cb2 = ChangeBatch::new_from(
                    it[0] as u64,
                    it[1] as i64,
                );
                j = ghost_indexes[20].1 as usize;
                cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

                // ---------------------------------------------------------------------------------- get the data
                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe { pc = *(area.offset(16 + 84 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(16 + 86 as isize) as *mut i64x2); }
                    dmb();
                    //println!("{} {}", pc[0], pc[1]);
                    //println!("{} {}", it[0], it[1]);
                }
// ---------------------------------------------------------------------------------- get the data
                #[cfg(feature = "no-fpga")] {
                    pc = i64x2::from_array([16, 16]);
                    it = i64x2::from_array([0, 0])
                }
// ----------------------------------------------------------------------------------- for the debug mode

                cb = ChangeBatch::new_from(time_1, pc[0] as i64 );
                cb1 = ChangeBatch::new_from(time_1, pc[1] as i64 );
                cb2 = ChangeBatch::new_from(
                    it[0] as u64,
                    it[1] as i64,
                );
                j = ghost_indexes[21].1 as usize;
                cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

                // ---------------------------------------------------------------------------------- get the data
                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe { pc = *(area.offset(16 + 88 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(16 + 90 as isize) as *mut i64x2); }
                    dmb();
                    //println!("{} {}", pc[0], pc[1]);
                    //println!("{} {}", it[0], it[1]);
                }
// ---------------------------------------------------------------------------------- get the data
                #[cfg(feature = "no-fpga")] {
                    pc = i64x2::from_array([16, 16]);
                    it = i64x2::from_array([0, 0])
                }
// ----------------------------------------------------------------------------------- for the debug mode

                cb = ChangeBatch::new_from(time_1, pc[0] as i64 );
                cb1 = ChangeBatch::new_from(time_1, pc[1] as i64 );
                cb2 = ChangeBatch::new_from(
                    it[0] as u64,
                    it[1] as i64,
                );
                j = ghost_indexes[22].1 as usize;
                cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

                // ---------------------------------------------------------------------------------- get the data
                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe { pc = *(area.offset(16 + 92 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(16 + 94 as isize) as *mut i64x2); }
                    dmb();
                    //println!("{} {}", pc[0], pc[1]);
                    //println!("{} {}", it[0], it[1]);
                }
// ---------------------------------------------------------------------------------- get the data
                #[cfg(feature = "no-fpga")] {
                    pc = i64x2::from_array([16, 16]);
                    it = i64x2::from_array([0, 0])
                }
// ----------------------------------------------------------------------------------- for the debug mode

                cb = ChangeBatch::new_from(time_1, pc[0] as i64 );
                cb1 = ChangeBatch::new_from(time_1, pc[1] as i64 );
                cb2 = ChangeBatch::new_from(
                    it[0] as u64,
                    it[1] as i64,
                );
                j = ghost_indexes[23].1 as usize;
                cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

                // ---------------------------------------------------------------------------------- get the data
                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe { pc = *(area.offset(16 + 96 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(16 + 98 as isize) as *mut i64x2); }
                    dmb();
                    //println!("{} {}", pc[0], pc[1]);
                    //println!("{} {}", it[0], it[1]);
                }
// ---------------------------------------------------------------------------------- get the data
                #[cfg(feature = "no-fpga")] {
                    pc = i64x2::from_array([16, 16]);
                    it = i64x2::from_array([0, 0])
                }
// ----------------------------------------------------------------------------------- for the debug mode

                cb = ChangeBatch::new_from(time_1, pc[0] as i64 );
                cb1 = ChangeBatch::new_from(time_1, pc[1] as i64 );
                cb2 = ChangeBatch::new_from(
                    it[0] as u64,
                    it[1] as i64,
                );
                j = ghost_indexes[24].1 as usize;
                cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

                // ---------------------------------------------------------------------------------- get the data
                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe { pc = *(area.offset(16 + 100 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(16 + 102 as isize) as *mut i64x2); }
                    dmb();
                    //println!("{} {}", pc[0], pc[1]);
                    //println!("{} {}", it[0], it[1]);
                }
// ---------------------------------------------------------------------------------- get the data
                #[cfg(feature = "no-fpga")] {
                    pc = i64x2::from_array([16, 16]);
                    it = i64x2::from_array([0, 0])
                }
// ----------------------------------------------------------------------------------- for the debug mode

                cb = ChangeBatch::new_from(time_1, pc[0] as i64 );
                cb1 = ChangeBatch::new_from(time_1, pc[1] as i64 );
                cb2 = ChangeBatch::new_from(
                    it[0] as u64,
                    it[1] as i64,
                );
                j = ghost_indexes[25].1 as usize;
                cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

                // ---------------------------------------------------------------------------------- get the data
                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe { pc = *(area.offset(16 + 104 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(16 + 106 as isize) as *mut i64x2); }
                    dmb();
                    //println!("{} {}", pc[0], pc[1]);
                    //println!("{} {}", it[0], it[1]);
                }
// ---------------------------------------------------------------------------------- get the data
                #[cfg(feature = "no-fpga")] {
                    pc = i64x2::from_array([16, 16]);
                    it = i64x2::from_array([0, 0])
                }
// ----------------------------------------------------------------------------------- for the debug mode

                cb = ChangeBatch::new_from(time_1, pc[0] as i64 );
                cb1 = ChangeBatch::new_from(time_1, pc[1] as i64 );
                cb2 = ChangeBatch::new_from(
                    it[0] as u64,
                    it[1] as i64,
                );
                j = ghost_indexes[26].1 as usize;
                cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

                // ---------------------------------------------------------------------------------- get the data
                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe { pc = *(area.offset(16 + 108 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(16 + 110 as isize) as *mut i64x2); }
                    dmb();
                    //println!("{} {}", pc[0], pc[1]);
                    //println!("{} {}", it[0], it[1]);
                }
// ---------------------------------------------------------------------------------- get the data
                #[cfg(feature = "no-fpga")] {
                    pc = i64x2::from_array([16, 16]);
                    it = i64x2::from_array([0, 0])
                }
// ----------------------------------------------------------------------------------- for the debug mode

                cb = ChangeBatch::new_from(time_1, pc[0] as i64 );
                cb1 = ChangeBatch::new_from(time_1, pc[1] as i64 );
                cb2 = ChangeBatch::new_from(
                    it[0] as u64,
                    it[1] as i64,
                );
                j = ghost_indexes[27].1 as usize;
                cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

                // ---------------------------------------------------------------------------------- get the data
                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe { pc = *(area.offset(16 + 112 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(16 + 114 as isize) as *mut i64x2); }
                    dmb();
                    //println!("{} {}", pc[0], pc[1]);
                    //println!("{} {}", it[0], it[1]);
                }
// ---------------------------------------------------------------------------------- get the data
                #[cfg(feature = "no-fpga")] {
                    pc = i64x2::from_array([16, 16]);
                    it = i64x2::from_array([0, 0])
                }
// ----------------------------------------------------------------------------------- for the debug mode

                cb = ChangeBatch::new_from(time_1, pc[0] as i64 );
                cb1 = ChangeBatch::new_from(time_1, pc[1] as i64 );
                cb2 = ChangeBatch::new_from(
                    it[0] as u64,
                    it[1] as i64,
                );
                j = ghost_indexes[28].1 as usize;
                cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

                // ---------------------------------------------------------------------------------- get the data
                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe { pc = *(area.offset(16 + 116 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(16 + 118 as isize) as *mut i64x2); }
                    dmb();
                    //println!("{} {}", pc[0], pc[1]);
                    //println!("{} {}", it[0], it[1]);
                }
// ---------------------------------------------------------------------------------- get the data
                #[cfg(feature = "no-fpga")] {
                    pc = i64x2::from_array([16, 16]);
                    it = i64x2::from_array([0, 0])
                }
// ----------------------------------------------------------------------------------- for the debug mode

                cb = ChangeBatch::new_from(time_1, pc[0] as i64 );
                cb1 = ChangeBatch::new_from(time_1, pc[1] as i64 );
                cb2 = ChangeBatch::new_from(
                    it[0] as u64,
                    it[1] as i64,
                );
                j = ghost_indexes[29].1 as usize;
                cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

                // ---------------------------------------------------------------------------------- get the data
                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe { pc = *(area.offset(16 + 120 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(16 + 122 as isize) as *mut i64x2); }
                    dmb();
                    //println!("{} {}", pc[0], pc[1]);
                    //println!("{} {}", it[0], it[1]);
                }
// ---------------------------------------------------------------------------------- get the data
                #[cfg(feature = "no-fpga")] {
                    pc = i64x2::from_array([16, 16]);
                    it = i64x2::from_array([0, 0])
                }
// ----------------------------------------------------------------------------------- for the debug mode

                cb = ChangeBatch::new_from(time_1, pc[0] as i64 );
                cb1 = ChangeBatch::new_from(time_1, pc[1] as i64 );
                cb2 = ChangeBatch::new_from(
                    it[0] as u64,
                    it[1] as i64,
                );
                j = ghost_indexes[30].1 as usize;
                cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                /*#[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe { pc = *(area.offset(76 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(78 as isize) as *mut i64x2); }
                    dmb();
                    //println!("{} {}", pc[0], pc[1]);
                    //println!("{} {}", it[0], it[1]);
                }
// ---------------------------------------------------------------------------------- get the data
                #[cfg(feature = "no-fpga")] {
                    pc = i64x2::from_array([16, 16]);
                    it = i64x2::from_array([0, 0]);
                }
// ----------------------------------------------------------------------------------- for the debug mode
                cb = ChangeBatch::new_from(time_1, pc[0] as i64 );
                cb1 = ChangeBatch::new_from(time_1, pc[1] as i64 );
                cb2 = ChangeBatch::new_from(
                    it[0] as u64,
                    it[1] as i64,
                );
                j = ghost_indexes[15].1 as usize;
                cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the
                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe { pc = *(area.offset(80 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(82 as isize) as *mut i64x2); }
                    dmb();
                    //println!("{} {}", pc[0], pc[1]);
                    //println!("{} {}", it[0], it[1]);
                }
// ---------------------------------------------------------------------------------- get the data
                #[cfg(feature = "no-fpga")] {
                    pc = i64x2::from_array([16, 16]);
                    it = i64x2::from_array([0, 0]);
                }
// ----------------------------------------------------------------------------------- for the debug mode

                cb = ChangeBatch::new_from(time_1, pc[0] as i64 );
                cb1 = ChangeBatch::new_from(time_1, pc[1] as i64 );
                cb2 = ChangeBatch::new_from(
                    it[0] as u64,
                    it[1] as i64,
                );
                j = ghost_indexes[16].1 as usize;
                cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe { pc = *(area.offset(84 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(86 as isize) as *mut i64x2); }
                    dmb();
                    //println!("{} {}", pc[0], pc[1]);
                    //println!("{} {}", it[0], it[1]);
                }
// ---------------------------------------------------------------------------------- get the data
                #[cfg(feature = "no-fpga")] {
                    pc = i64x2::from_array([16, 16]);
                    it = i64x2::from_array([0, 0]);
                }
// ----------------------------------------------------------------------------------- for the debug mode

                cb = ChangeBatch::new_from(time_1, pc[0] as i64 );
                cb1 = ChangeBatch::new_from(time_1, pc[1] as i64 );
                cb2 = ChangeBatch::new_from(
                    it[0] as u64,
                    it[1] as i64,
                );
                j = ghost_indexes[17].1 as usize;
                cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe { pc = *(area.offset(88 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(90 as isize) as *mut i64x2); }
                    dmb();
                    //println!("{} {}", pc[0], pc[1]);
                    //println!("{} {}", it[0], it[1]);
                }
// ---------------------------------------------------------------------------------- get the data
                #[cfg(feature = "no-fpga")] {
                    pc = i64x2::from_array([16, 16]);
                    it = i64x2::from_array([0, 0]);
                }
// ----------------------------------------------------------------------------------- for the debug mode

                cb = ChangeBatch::new_from(time_1, pc[0] as i64 );
                cb1 = ChangeBatch::new_from(time_1, pc[1] as i64 );
                cb2 = ChangeBatch::new_from(
                    it[0] as u64,
                    it[1] as i64,
                );
                j = ghost_indexes[18].1 as usize;
                cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);


// ---------------------------------------------------------------------------------- get the data
                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe { pc = *(area.offset(92 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(94 as isize) as *mut i64x2); }
                    dmb();
                    //println!("{} {}", pc[0], pc[1]);
                    //println!("{} {}", it[0], it[1]);
                }
// ---------------------------------------------------------------------------------- get the data
                #[cfg(feature = "no-fpga")] {
                    pc = i64x2::from_array([16, 16]);
                    it = i64x2::from_array([0, 0])
                }
// ----------------------------------------------------------------------------------- for the debug mode

                cb = ChangeBatch::new_from(time_1, pc[0] as i64 );
                cb1 = ChangeBatch::new_from(time_1, pc[1] as i64 );
                cb2 = ChangeBatch::new_from(
                    it[0] as u64,
                    it[1] as i64,
                );
                j = ghost_indexes[19].1 as usize;
                cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
*/
            }

            //let epoch_end = Instant::now();
            //let total_nanos = (epoch_end - epoch_start).as_nanos();
            //println!("wrapper latency: {total_nanos}");

            if !has_data {
                //println!("No data");


                let area = unsafe { (*hc).area } as *mut u64;

                let mut v1: Vec<u64x2> = Vec::new();
                let mut v0: Vec<u64x2> = Vec::new();

                let mut current_length = 0;

                let data_length = num_data;

                /*for i in (0 .. borrow.len()).step_by(2) {
                    let frontier1 = borrow[i].borrow().frontier();
                    let frontier2 = borrow[i+1].borrow().frontier();
                    /*if frontier.len() == 0 {
                        let x =  u64x2::from_array([0, 0]);
                        v0.push(x);
                        current_length += 2;
                    } else if frontier.len() == 1 {
                        let x =  u64x2::from_array([(frontier[0] << 1) | 1u64, 0]);
                        v0.push(x);
                        current_length += 2;

                    } else {
                        for val in (0..frontier.len()).step_by(2) {
                            let x =  u64x2::from_array([(frontier[val] << 1) | 1u64, (frontier[val+1] << 1) | 1u64]);
                            v0.push(x);
                            current_length += 2;
                        }
                    }*/

                    if (frontier1.len() == 0 && frontier2.len() == 0) {
                        let x =  u64x2::from_array([0, 0]);
                        v0.push(x);
                        current_length += 2;
                    } else {
                        let x =  u64x2::from_array([(frontier1[0] << 1) | 1u64, (frontier2[0] << 1) | 1u64]);
                        v0.push(x);
                        current_length += 2;
                    }
                }

                //println!("Push frontiers");

                for i in (current_length..frontier_length).step_by(2) {
                    let x =  u64x2::from_array([0, 0]);
                    v0.push(x);
                    current_length += 2;
                }*/

                //println!("Current length = {}", current_length);
                //
                for i in (0..29).step_by(2) {
                    let frontier1 = borrow[i].borrow().frontier();
                    let frontier2 = borrow[i+1].borrow().frontier();
                    /*if frontier.len() == 0 {
                        let x =  u64x2::from_array([0, 0]);
                        v0.push(x);
                        current_length += 2;
                    } else {
                        for val in (0..frontier.len()).step_by(2) {
                            let x =  u64x2::from_array([(frontier[val] << 1) | 1u64, (frontier[val] << 1) | 1u64]);
                            v0.push(x);
                            current_length += 2;
                        }
                    }*/
                    // for now we will assume that frontier has length 1, if it is not 1 , then we already might want to modify logic on the
                    // FPGA side as well
                    let x =  u64x2::from_array([(frontier1[0] << 1) | 1u64, (frontier2[0] << 1) | 1u64]);
                    v0.push(x);
                    current_length += 2;
                }



                let frontier_last = borrow[30].borrow().frontier();
                let x =  u64x2::from_array([(frontier_last[0] << 1) | 1u64, 0]);
                v0.push(x);

                //println!("Current length = {}", current_length);

                for i in (0..16).step_by(2) {
                    let x = u64x2::from_array([0, 0]);
                    v0.push(x);
                }

//--------------------------------------------------------------------------------------------- print the output data
                /*println!("OUTPUT DATA FROM TIMELY");
                println!("Length of frontier vector {}", v0.len());
                for val in &v0 {
                    println!("{} {}", val[0], val[1]);
                }
                println!();

                println!("Length of data vector {}", v1.len());
                for val in &v1 {
                    println!("{} {}", val[0], val[1]);
                }
                println!();*/
//--------------------------------------------------------------------------------------------- print the output data

                #[cfg(not(feature = "no-fpga"))] {
                    //dmb();
                    unsafe{*(area.offset(0 as isize) as *mut u64x2) = v0[0]};
                    unsafe{*(area.offset(2 as isize) as *mut u64x2) = v0[1]};
                    unsafe{*(area.offset(4 as isize) as *mut u64x2) = v0[2]};
                    unsafe{*(area.offset(6 as isize) as *mut u64x2) = v0[3]};
                    unsafe{*(area.offset(8 as isize) as *mut u64x2) = v0[4]};
                    unsafe{*(area.offset(10 as isize) as *mut u64x2) = v0[5]};
                    unsafe{*(area.offset(12 as isize) as *mut u64x2) = v0[6]};
                    unsafe{*(area.offset(14 as isize) as *mut u64x2) = v0[7]};
                    unsafe{*(area.offset(16 as isize) as *mut u64x2) = v0[8]};
                    unsafe{*(area.offset(18 as isize) as *mut u64x2) = v0[9]};
                    unsafe{*(area.offset(20 as isize) as *mut u64x2) = v0[10]};
                    unsafe{*(area.offset(22 as isize) as *mut u64x2) = v0[11]};
                    unsafe{*(area.offset(24 as isize) as *mut u64x2) = v0[12]};
                    unsafe{*(area.offset(26 as isize) as *mut u64x2) = v0[13]};
                    unsafe{*(area.offset(28 as isize) as *mut u64x2) = v0[14]};
                    unsafe{*(area.offset(30 as isize) as *mut u64x2) = v0[15]};
                    unsafe{*(area.offset(32 as isize) as *mut u64x2) = v0[16]};
                    unsafe{*(area.offset(34 as isize) as *mut u64x2) = v0[17]};
                    unsafe{*(area.offset(36 as isize) as *mut u64x2) = v0[18]};
                    unsafe{*(area.offset(38 as isize) as *mut u64x2) = v0[19]};
                    unsafe{*(area.offset(40 as isize) as *mut u64x2) = v0[20]};
                    unsafe{*(area.offset(42 as isize) as *mut u64x2) = v0[21]};
                    unsafe{*(area.offset(44 as isize) as *mut u64x2) = v0[22]};
                    unsafe{*(area.offset(46 as isize) as *mut u64x2) = v0[23]};
                    dmb();
                }


                let mut pc = Vec::with_capacity(200);//i64x2::from_array([0 , 0]);
                unsafe{pc.set_len(200);}
                //let mut it = Vec::with_capacity(50);//i64x2::from_array([0 , 0]);
                let mut data: u64x2 = u64x2::from_array([0 , 0]);
                let mut pc2: i64x2 = i64x2::from_array([0 , 0]);





                //#[cfg(not(feature = "no-fpga"))] {
                //dmb();
                for i in (0..data_length).step_by(2) {
                    //dmb();
                    unsafe { data = *(area.offset(i as isize) as *mut u64x2); }

                    // all the writes can be done asynchronously
                    // we are getting two numbers here
                    // the offset for progress would be 18
                    dmb();
                    let shifted_val1 = data[0] >> 1;
                    let shifted_val2 = data[1] >> 1;
                    if data[0] != 0 {
                        vector2.push(shifted_val1);
                    }
                    if data[1] != 0 {
                        vector2.push(shifted_val2);
                    }
                }
                //dmb();
                //}
                //
                /*println!("INPUT DATA TO TIMELY");
                println!("Length of frontier vector {}", v0.len());
                for val in &vector2 {
                    print!("{} ", val);
                }
                println!();*/

                let id_wrap = ghost_indexes[ghost_indexes.len() - 1].1;

                let mut cc = 0;
                for i in (16..139).step_by(2) {
                    //dmb();
                    unsafe { pc[cc] = *(area.offset(i as isize) as *mut i64x2); }
                    //println!("{} {} \n", pc2[0], pc2[1]);
                    dmb();
                    cc += 1;
                    //println!("cc={}", cc);
                }


                // we are even not reading updates if vector is equal to 0
                // Alright: that is not necessarily true, what needs to be done I think is that there can
                // be also some output for example from some window that got filtered out by the filter afterwards
                // we still need all the updates, not only if the end result
                // but for now we can leavelike this
                // if there is something we want to release to the world this would be the last result
                // but for the intermediate results we just need to get internals
                // I should set for now some placeholder for time and potentially replace it with something meaningful
                // in initial verison I was providing data only for the id wrap
                // which I assume was the wrapping around the window
                // so not for all of the operators but
                // so basically in theory I should read only that part
                // in theory this was not really discussed yet but in general I just need to fetch
                // only needed data
                // or fetch everything
                // in any case I need to fetch everything and pick a random time
                // as this case will never be true for now as I don't simply have a use case for that

                // how it should be done:
                // 1) mark all the nodes that can produce output
                // 2) read them // if there is nothing do not insert anything
                // 3) oh, really: check against 0, if all 0, don't do anything otherwise insert
                // 4) if output is zero then don't insert anything
                // this approach is a bit unfortunate in terms of cache lines
                // in any case I can test later if something changes if there are just zeros
                // entering the algorithm



                if vector2.len() > 0 {

                    println!("Vector is not empty");

                    let mut k = 0;
                    let mut i = 0 as usize;
                    let mut j = 0;
                    let mut cb = ChangeBatch::new_from(0, 0);
                    let mut cb1 = ChangeBatch::new_from(0, 0);
                    let mut cb2 = ChangeBatch::new_from(0, 0);
                    let mut counter_offset = 0;

                    let time_1 = pc[1][0] as u64;

// ---------------------------------------------------------------------------------- get the data
                    /*                  unsafe { pc = *(area.offset(16 as isize) as *mut i64x2); }
                                        dmb();
                                        unsafe { it = *(area.offset(18 as isize) as *mut i64x2); }
                                        dmb();*/

// ---------------------------------------------------------------------------------- got data
                    cb = ChangeBatch::new_from(time_1, pc[0][0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[0][1] as i64);
                    cb2 = ChangeBatch::new_from(
                        pc[1][0] as u64,
                        pc[1][1] as i64,
                    );
                    j = ghost_indexes[0].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                    /*unsafe { pc = *(area.offset(20 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(22 as isize) as *mut i64x2); }*/
// ---------------------------------------------------------------------------------- got data
                    cb = ChangeBatch::new_from(time_1, pc[2][0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[2][1] as i64);
                    cb2 = ChangeBatch::new_from(
                        pc[3][0] as u64,
                        pc[3][1] as i64,
                    );
                    j = ghost_indexes[1].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                    /* unsafe { pc = *(area.offset(24 as isize) as *mut i64x2); }
                     dmb();
                     unsafe { it = *(area.offset(26 as isize) as *mut i64x2); }*/
// ---------------------------------------------------------------------------------- got data
                    cb = ChangeBatch::new_from(time_1, pc[4][0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[4][1] as i64);
                    cb2 = ChangeBatch::new_from(
                        pc[5][0] as u64,
                        pc[5][1] as i64,
                    );

                    j = ghost_indexes[2].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                    /* unsafe { pc = *(area.offset(28 as isize) as *mut i64x2); }
                     dmb();
                     unsafe { it = *(area.offset(30 as isize) as *mut i64x2); }*/
// ---------------------------------------------------------------------------------- got data
                    cb = ChangeBatch::new_from(time_1, pc[6][0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[6][1] as i64);
                    cb2 = ChangeBatch::new_from(
                        pc[7][0] as u64,
                        pc[7][1] as i64,
                    );
                    j = ghost_indexes[3].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
                    //println!("DONE 4");

// ---------------------------------------------------------------------------------- get the data
                    /*unsafe { pc = *(area.offset(32 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(34 as isize) as *mut i64x2); }*/
// ---------------------------------------------------------------------------------- got data
                    cb = ChangeBatch::new_from(time_1, pc[8][0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[8][1] as i64);
                    cb2 = ChangeBatch::new_from(
                        pc[9][0] as u64,
                        pc[9][1] as i64,
                    );
                    j = ghost_indexes[4].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                    /*  unsafe { pc = *(area.offset(36 as isize) as *mut i64x2); }
                      dmb();
                      unsafe { it = *(area.offset(38 as isize) as *mut i64x2); }
                      dmb();*/
// ---------------------------------------------------------------------------------- got data

                    cb = ChangeBatch::new_from(time_1, pc[10][0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[10][1] as i64);
                    cb2 = ChangeBatch::new_from(
                        pc[11][0] as u64,
                        pc[11][1] as i64,
                    );
                    j = ghost_indexes[5].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                    /*unsafe { pc = *(area.offset(40 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(42 as isize) as *mut i64x2); }
                    dmb();*/
// ---------------------------------------------------------------------------------- get the data

                    cb = ChangeBatch::new_from(time_1, pc[12][0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[12][1] as i64);
                    cb2 = ChangeBatch::new_from(
                        pc[13][0] as u64,
                        pc[13][1] as i64,
                    );

                    j = ghost_indexes[6].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                    /*unsafe { pc = *(area.offset(44 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(46 as isize) as *mut i64x2); }
                    dmb();*/

// ---------------------------------------------------------------------------------- get the data
                    cb = ChangeBatch::new_from(time_1, pc[14][0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[14][1] as i64);
                    cb2 = ChangeBatch::new_from(
                        pc[15][0] as u64,
                        pc[15][1] as i64,
                    );
                    j = ghost_indexes[7].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
                    i = 0;
                    //println!("DONE 5");

// ---------------------------------------------------------------------------------- get the data
                    /* unsafe { pc = *(area.offset(48 as isize) as *mut i64x2); }
                     dmb();
                     unsafe { it = *(area.offset(50 as isize) as *mut i64x2); }
                     dmb();*/
// ---------------------------------------------------------------------------------- get the data

                    cb = ChangeBatch::new_from(time_1, pc[16][0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[16][1] as i64);
                    cb2 = ChangeBatch::new_from(
                        pc[17][0] as u64,
                        pc[17][1] as i64,
                    );
                    j = ghost_indexes[8].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                    /*unsafe { pc = *(area.offset(52 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(54 as isize) as *mut i64x2); }
                    dmb();*/
// ---------------------------------------------------------------------------------- get the data

                    cb = ChangeBatch::new_from(time_1, pc[18][0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[18][1] as i64);
                    cb2 = ChangeBatch::new_from(
                        pc[19][0] as u64,
                        pc[19][1] as i64,
                    );
                    j = ghost_indexes[9].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
                    i = i + 4;

// ---------------------------------------------------------------------------------- get the data
                    /* unsafe { pc = *(area.offset(56 as isize) as *mut i64x2); }
                     dmb();
                     unsafe { it = *(area.offset(58 as isize) as *mut i64x2); }
                     dmb();*/
// ---------------------------------------------------------------------------------- get the data
                    cb = ChangeBatch::new_from(time_1, pc[20][0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[20][1] as i64);
                    cb2 = ChangeBatch::new_from(
                        pc[21][0] as u64,
                        pc[21][1] as i64,
                    );

                    j = ghost_indexes[10].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                    /* unsafe { pc = *(area.offset(60 as isize) as *mut i64x2); }
                     dmb();
                     unsafe { it = *(area.offset(62 as isize) as *mut i64x2); }
                     dmb();*/
// ---------------------------------------------------------------------------------- get the data

                    cb = ChangeBatch::new_from(time_1, pc[22][0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[22][1] as i64);
                    cb2 = ChangeBatch::new_from(
                        pc[23][0] as u64,
                        pc[23][1] as i64,
                    );
                    j = ghost_indexes[11].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
                    //println!("DONE 6");

// ---------------------------------------------------------------------------------- get the data
                    /*unsafe { pc = *(area.offset(64 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(66 as isize) as *mut i64x2); }
                    dmb();*/
// ---------------------------------------------------------------------------------- get the data
                    cb = ChangeBatch::new_from(time_1, pc[24][0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[24][1] as i64);
                    cb2 = ChangeBatch::new_from(
                        pc[25][0] as u64,
                        pc[25][1] as i64,
                    );
                    j = ghost_indexes[12].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                    /* unsafe { pc = *(area.offset(68 as isize) as *mut i64x2); }
                     dmb();
                     unsafe { it = *(area.offset(70 as isize) as *mut i64x2); }
                     dmb();*/
// ---------------------------------------------------------------------------------- get the data

                    cb = ChangeBatch::new_from(time_1, pc[26][0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[26][1] as i64);
                    cb2 = ChangeBatch::new_from(
                        pc[27][0] as u64,
                        pc[27][1] as i64,
                    );
                    j = ghost_indexes[13].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                    /*unsafe { pc = *(area.offset(72 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(74 as isize) as *mut i64x2); }
                    dmb();*/
// ---------------------------------------------------------------------------------- get the data

                    cb = ChangeBatch::new_from(time_1, pc[28][0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[28][1] as i64);
                    cb2 = ChangeBatch::new_from(
                        pc[29][0] as u64,
                        pc[29][1] as i64,
                    );
                    j = ghost_indexes[14].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

                    // ---------------------------------------------------------------------------------- get the data
                    /*unsafe { pc = *(area.offset(76 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(78 as isize) as *mut i64x2); }
                    dmb();*/
// ---------------------------------------------------------------------------------- get the data
                    cb = ChangeBatch::new_from(time_1, pc[30][0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[30][1] as i64);
                    cb2 = ChangeBatch::new_from(
                        pc[31][0] as u64,
                        pc[31][1] as i64,
                    );
                    j = ghost_indexes[15].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                    /*unsafe { pc = *(area.offset(80 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(82 as isize) as *mut i64x2); }
                    dmb();*/
// ---------------------------------------------------------------------------------- get the data

                    cb = ChangeBatch::new_from(time_1, pc[32][0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[32][1] as i64);
                    cb2 = ChangeBatch::new_from(
                        pc[33][0] as u64,
                        pc[33][1] as i64,
                    );
                    j = ghost_indexes[16].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                    /*unsafe { pc = *(area.offset(84 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(86 as isize) as *mut i64x2); }
                    dmb();*/
// ---------------------------------------------------------------------------------- get the data

                    cb = ChangeBatch::new_from(time_1, pc[34][0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[34][1] as i64);
                    cb2 = ChangeBatch::new_from(
                        pc[35][0] as u64,
                        pc[35][1] as i64,
                    );
                    j = ghost_indexes[17].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                    /*unsafe { pc = *(area.offset(88 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(90 as isize) as *mut i64x2); }
                    dmb();*/
// ---------------------------------------------------------------------------------- get the data

                    cb = ChangeBatch::new_from(time_1, pc[36][0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[36][1] as i64);
                    cb2 = ChangeBatch::new_from(
                        pc[37][0] as u64,
                        pc[37][1] as i64,
                    );
                    j = ghost_indexes[18].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                    /*unsafe { pc = *(area.offset(92 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(94 as isize) as *mut i64x2); }
                    dmb();*/
// ---------------------------------------------------------------------------------- get the data

                    cb = ChangeBatch::new_from(time_1, pc[38][0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[38][1] as i64);
                    cb2 = ChangeBatch::new_from(
                        pc[39][0] as u64,
                        pc[39][1] as i64,
                    );
                    j = ghost_indexes[19].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                    /*#[cfg(feature = "no-fpga")] {
                        pc = i64x2::from_array([16, 16]);
                        it = i64x2::from_array([0, 0])
                    }*/
// ----------------------------------------------------------------------------------- for the debug mode

                    cb = ChangeBatch::new_from(time_1, pc[40][0] as i64 );
                    cb1 = ChangeBatch::new_from(time_1, pc[40][1] as i64 );
                    cb2 = ChangeBatch::new_from(
                        pc[41][0] as u64,
                        pc[41][1] as i64,
                    );
                    j = ghost_indexes[20].1 as usize;
                    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                    /*#[cfg(feature = "no-fpga")] {
                        pc = i64x2::from_array([16, 16]);
                        it = i64x2::from_array([0, 0])
                    }*/
// ----------------------------------------------------------------------------------- for the debug mode

                    cb = ChangeBatch::new_from(time_1, pc[42][0] as i64 );
                    cb1 = ChangeBatch::new_from(time_1, pc[42][1] as i64 );
                    cb2 = ChangeBatch::new_from(
                        pc[43][0] as u64,
                        pc[43][1] as i64,
                    );
                    j = ghost_indexes[21].1 as usize;
                    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                    /*#[cfg(feature = "no-fpga")] {
                        pc = i64x2::from_array([16, 16]);
                        it = i64x2::from_array([0, 0])
                    }*/
// ----------------------------------------------------------------------------------- for the debug mode

                    cb = ChangeBatch::new_from(time_1, pc[44][0] as i64 );
                    cb1 = ChangeBatch::new_from(time_1, pc[44][1] as i64 );
                    cb2 = ChangeBatch::new_from(
                        pc[45][0] as u64,
                        pc[45][1] as i64,
                    );
                    j = ghost_indexes[22].1 as usize;
                    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                    /*#[cfg(feature = "no-fpga")] {
                        pc = i64x2::from_array([16, 16]);
                        it = i64x2::from_array([0, 0])
                    }*/
// ----------------------------------------------------------------------------------- for the debug mode

                    cb = ChangeBatch::new_from(time_1, pc[46][0] as i64 );
                    cb1 = ChangeBatch::new_from(time_1, pc[46][1] as i64 );
                    cb2 = ChangeBatch::new_from(
                        pc[47][0] as u64,
                        pc[47][1] as i64,
                    );
                    j = ghost_indexes[23].1 as usize;
                    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                    /*#[cfg(feature = "no-fpga")] {
                        pc = i64x2::from_array([16, 16]);
                        it = i64x2::from_array([0, 0])
                    }*/
// ----------------------------------------------------------------------------------- for the debug mode

                    cb = ChangeBatch::new_from(time_1, pc[48][0] as i64 );
                    cb1 = ChangeBatch::new_from(time_1, pc[48][1] as i64 );
                    cb2 = ChangeBatch::new_from(
                        pc[49][0] as u64,
                        pc[49][1] as i64,
                    );
                    j = ghost_indexes[24].1 as usize;
                    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                    /*#[cfg(feature = "no-fpga")] {
                        pc = i64x2::from_array([16, 16]);
                        it = i64x2::from_array([0, 0])
                    }*/
// ----------------------------------------------------------------------------------- for the debug mode

                    cb = ChangeBatch::new_from(time_1, pc[50][0] as i64 );
                    cb1 = ChangeBatch::new_from(time_1, pc[50][1] as i64 );
                    cb2 = ChangeBatch::new_from(
                        pc[51][0] as u64,
                        pc[51][1] as i64,
                    );
                    j = ghost_indexes[25].1 as usize;
                    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                    /*#[cfg(feature = "no-fpga")] {
                        pc = i64x2::from_array([16, 16]);
                        it = i64x2::from_array([0, 0])
                    }*/
// ----------------------------------------------------------------------------------- for the debug mode

                    cb = ChangeBatch::new_from(time_1, pc[52][0] as i64 );
                    cb1 = ChangeBatch::new_from(time_1, pc[52][1] as i64 );
                    cb2 = ChangeBatch::new_from(
                        pc[53][0] as u64,
                        pc[53][1] as i64,
                    );
                    j = ghost_indexes[26].1 as usize;
                    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                    /*#[cfg(feature = "no-fpga")] {
                        pc = i64x2::from_array([16, 16]);
                        it = i64x2::from_array([0, 0])
                    }*/
// ----------------------------------------------------------------------------------- for the debug mode

                    cb = ChangeBatch::new_from(time_1, pc[54][0] as i64 );
                    cb1 = ChangeBatch::new_from(time_1, pc[54][1] as i64 );
                    cb2 = ChangeBatch::new_from(
                        pc[55][0] as u64,
                        pc[55][1] as i64,
                    );
                    j = ghost_indexes[27].1 as usize;
                    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                    /*#[cfg(feature = "no-fpga")] {
                        pc = i64x2::from_array([16, 16]);
                        it = i64x2::from_array([0, 0])
                    }*/
// ----------------------------------------------------------------------------------- for the debug mode

                    cb = ChangeBatch::new_from(time_1, pc[56][0] as i64 );
                    cb1 = ChangeBatch::new_from(time_1, pc[56][1] as i64 );
                    cb2 = ChangeBatch::new_from(
                        pc[57][0] as u64,
                        pc[57][1] as i64,
                    );
                    j = ghost_indexes[28].1 as usize;
                    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);


// ---------------------------------------------------------------------------------- get the data
                    /*#[cfg(feature = "no-fpga")] {
                        pc = i64x2::from_array([16, 16]);
                        it = i64x2::from_array([0, 0])
                    }*/
// ----------------------------------------------------------------------------------- for the debug mode

                    cb = ChangeBatch::new_from(time_1, pc[58][0] as i64 );
                    cb1 = ChangeBatch::new_from(time_1, pc[58][1] as i64 );
                    cb2 = ChangeBatch::new_from(
                        pc[59][0] as u64,
                        pc[59][1] as i64,
                    );
                    j = ghost_indexes[29].1 as usize;
                    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                    /*#[cfg(feature = "no-fpga")] {
                        pc = i64x2::from_array([16, 16]);
                        it = i64x2::from_array([0, 0])
                    }*/
// ----------------------------------------------------------------------------------- for the debug mode

                    cb = ChangeBatch::new_from(time_1, pc[60][0] as i64 );
                    cb1 = ChangeBatch::new_from(time_1, pc[60][1] as i64 );
                    cb2 = ChangeBatch::new_from(
                        pc[61][0] as u64,
                        pc[61][1] as i64,
                    );
                    j = ghost_indexes[30].1 as usize;
                    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                    /*unsafe { pc = *(area.offset(76 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(78 as isize) as *mut i64x2); }
                    dmb();*/
// ---------------------------------------------------------------------------------- get the data
                    /*cb = ChangeBatch::new_from(time_1, pc[30][0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[30][1] as i64);
                    cb2 = ChangeBatch::new_from(
                        pc[31][0] as u64,
                        pc[31][1] as i64,
                    );
                    j = ghost_indexes[15].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                    /*unsafe { pc = *(area.offset(80 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(82 as isize) as *mut i64x2); }
                    dmb();*/
// ---------------------------------------------------------------------------------- get the data

                    cb = ChangeBatch::new_from(time_1, pc[32][0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[32][1] as i64);
                    cb2 = ChangeBatch::new_from(
                        pc[33][0] as u64,
                        pc[33][1] as i64,
                    );
                    j = ghost_indexes[16].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                    /*unsafe { pc = *(area.offset(84 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(86 as isize) as *mut i64x2); }
                    dmb();*/
// ---------------------------------------------------------------------------------- get the data

                    cb = ChangeBatch::new_from(time_1, pc[34][0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[34][1] as i64);
                    cb2 = ChangeBatch::new_from(
                        pc[35][0] as u64,
                        pc[35][1] as i64,
                    );
                    j = ghost_indexes[17].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                    /*unsafe { pc = *(area.offset(88 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(90 as isize) as *mut i64x2); }
                    dmb();*/
// ---------------------------------------------------------------------------------- get the data

                    cb = ChangeBatch::new_from(time_1, pc[36][0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[36][1] as i64);
                    cb2 = ChangeBatch::new_from(
                        pc[37][0] as u64,
                        pc[37][1] as i64,
                    );
                    j = ghost_indexes[18].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);

// ---------------------------------------------------------------------------------- get the data
                    /*unsafe { pc = *(area.offset(92 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(94 as isize) as *mut i64x2); }
                    dmb();*/
// ---------------------------------------------------------------------------------- get the data

                    cb = ChangeBatch::new_from(time_1, pc[38][0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[38][1] as i64);
                    cb2 = ChangeBatch::new_from(
                        pc[39][0] as u64,
                        pc[39][1] as i64,
                    );
                    j = ghost_indexes[19].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
*/
                    output_wrapper
                        .session(&time_1)
                        .give_vec(&mut vector2);
                }


            }

            //let epoch_end = Instant::now();
            // let total_nanos = (epoch_end - epoch_start).as_nanos();
            // println!("wrapper latency latency: {total_nanos}");

            vector.clear();
            vector2.clear();
            output_wrapper.cease();

            //let epoch_end = Instant::now();
            //let total_nanos = (epoch_end - epoch_start).as_nanos();
            //println!("wrapper latency latency: {total_nanos}");
            //println!("Finished!");

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

        for builder_filter in vec_builder_filter {
            ghost_operators.push(builder_filter.index());
        }

        /*for builder_map in vec_builder_map {
            ghost_operators.push(builder_map.index());
        }*/

        // Acquire handle to shared progress
        let shared_progress = Rc::new(RefCell::new(SharedProgress::new_ghosts(
            builder_wrapper.shape().inputs(),
            builder_wrapper.shape().outputs(),
            ghost_operators.clone(),
        )));

        builder_wrapper.set_notify(false);
        let operator = FpgaOperator {
            shape: builder_wrapper.shape().clone(),
            address: builder_wrapper.address().clone(),
            activations: self.scope().activations().clone(),
            logic: raw_logic,
            shared_progress: Rc::clone(&shared_progress),
            summary: builder_wrapper.summary().to_vec(),
            ghost_indexes: ghost_indexes2,
        };

        // add fpga operator to scope
        self.scope().add_operator_with_indices(
            Box::new(operator),
            builder_wrapper.index(),
            builder_wrapper.global(),
        );

        // we also need to create a map from ghost to wrapper
        self.scope().add_fpga_operator(
            builder_wrapper.index(),
            ghost_operators.clone(),
            ghost_edges.clone(),
        );

        return stream_wrapper;
    }
}
