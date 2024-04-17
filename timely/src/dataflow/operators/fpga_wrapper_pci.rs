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
pub struct HardwareCommon {
    area: * mut c_void
}


unsafe impl Send for HardwareCommon{}
unsafe impl Sync for HardwareCommon{}

/*#[link(name = "pci_shim")]
extern "C" {
    fn run(hc: * const HardwareCommon, input_size: i64, output_size: i64);
}*/

/*unsafe fn my_run(hc: * const HardwareCommon/*, input_size: i64, output_size: i64*/) {

    let area = unsafe { (*hc).area } as *mut u64;

    let data_i = vec![1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
                      21, 21, 21, 21, 21, 21, 21, 21,
                      25, 25, 25, 25, 25, 25, 25, 25];

    let mut v1: Vec<i64x2> = Vec::new();

    for i in (0..39).step_by(2) {
        let x = i64x2::from_array([data_i[i], data_i[i+1]]);
        v1.push(x);
    }

    unsafe{*(area.offset(0 as isize) as *mut i64x2) = v1[0]};
    unsafe{*(area.offset(2 as isize) as *mut i64x2) = v1[1]};
    unsafe{*(area.offset(4 as isize) as *mut i64x2) = v1[2]};
    unsafe{*(area.offset(6 as isize) as *mut i64x2) = v1[3]};
    unsafe{*(area.offset(8 as isize) as *mut i64x2) = v1[4]};
    unsafe{*(area.offset(10 as isize) as *mut i64x2) = v1[5]};
    unsafe{*(area.offset(12 as isize) as *mut i64x2) = v1[6]};
    unsafe{*(area.offset(14 as isize) as *mut i64x2) = v1[7]};
    unsafe{*(area.offset(16 as isize) as *mut i64x2) = v1[8]};
    unsafe{*(area.offset(18 as isize) as *mut i64x2) = v1[9]};
    unsafe{*(area.offset(20 as isize) as *mut i64x2) = v1[10]};
    unsafe{*(area.offset(22 as isize) as *mut i64x2) = v1[11]};
    unsafe{*(area.offset(24 as isize) as *mut i64x2) = v1[12]};
    unsafe{*(area.offset(26 as isize) as *mut i64x2) = v1[13]};
    unsafe{*(area.offset(28 as isize) as *mut i64x2) = v1[14]};
    unsafe{*(area.offset(30 as isize) as *mut i64x2) = v1[15]};
    unsafe{*(area.offset(32 as isize) as *mut i64x2) = v1[16]};
    unsafe{*(area.offset(34 as isize) as *mut i64x2) = v1[17]};
    unsafe{*(area.offset(36 as isize) as *mut i64x2) = v1[18]};
    unsafe{*(area.offset(38 as isize) as *mut i64x2) = v1[19]};
    dmb();

}*/


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

        let mut frontier_length = 16;//(num_operators / CACHE_LINE_SIZE) + CACHE_LINE_SIZE;
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

        let mut produced = HashMap::with_capacity(32);
        let mut consumed = HashMap::with_capacity(32);
        let mut internals = HashMap::with_capacity(32);

        /*let mut offset_1 = 0;
        let mut offset_2 = 0;

        get_offset(&mut offset_1, &mut offset_2);


        let area = unsafe { (*hc).area } as *mut u64;
        let cache_line_1 = unsafe { std::slice::from_raw_parts_mut(area.offset(offset_1.try_into().unwrap()), CACHE_LINE_SIZE as usize) };
        let cache_line_2 = unsafe {
            std::slice::from_raw_parts_mut(
                area.offset(offset_2.try_into().unwrap()),
                CACHE_LINE_SIZE as usize,
            )
        };*/


        let raw_logic = move |progress: &mut SharedProgress<S::Timestamp>| {

            let epoch_start = Instant::now();

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

                let mut current_length = 0;


                let area = unsafe { (*hc).area } as *mut u64;

                let mut v1: Vec<i64x2> = Vec::new();
                let mut v0: Vec<u64x2> = Vec::new();

                current_length += 1;

                for i in 0 .. borrow.len() {
                    let frontier = borrow[i].borrow().frontier();
                    if frontier.len() == 0 {
                        *memory.offset(current_length as isize) = 0;
                        current_length += 1;
                    } else {
                        for val in (0..frontier.len()).step_by(2) {
                            let x =  u64x2::from_array([(frontier[val] << 1) | 1i64, (frontier[val] << 1) | 1i64]);
                            v0.push(x);
                            //current_length += 1;
                        }
                    }
                }

                for i in (current_length..max_length_in).step_by(2) {
                    let x =  u64x2::from_array([0, 0]);
                    v0.push(x);
                }

                for i in (0..16).step_by(2) {
                    let x = i64x2::from_array([data_i[i], data_i[i+1]]);
                    v1.push(x);
                }

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
                unsafe{*(area.offset(24 as isize) as *mut i64x2) = v1[12]};
                unsafe{*(area.offset(26 as isize) as *mut i64x2) = v1[13]};
                unsafe{*(area.offset(28 as isize) as *mut i64x2) = v1[14]};
                unsafe{*(area.offset(30 as isize) as *mut i64x2) = v1[15]};
                unsafe{*(area.offset(32 as isize) as *mut i64x2) = v1[16]};
                unsafe{*(area.offset(34 as isize) as *mut i64x2) = v1[17]};
                unsafe{*(area.offset(36 as isize) as *mut i64x2) = v1[18]};
                unsafe{*(area.offset(38 as isize) as *mut i64x2) = v1[19]};
                dmb();


                let mut pc: i64x2 = i64x2::from_array([0 , 0]);
                let mut it: i64x2 = i64x2::from_array([0 , 0]);

                for i in (0 .. data_length).step_by(2) {
                    unsafe{pc = *(area.offset(i as isize) as *mut i64x2);}
                    // all the writes can be done asynchronously
                    // we are getting two numbers here
                    // the offset for progress would be 18
                    dmb();
                    let shifted_val1 = pc[0] >> 1;
                    let shifted_val2 = pc[1] >> 1;
                    if val1 != 0 {
                        vector2.push(shifted_val1);
                    }
                    if val2 != 0 {
                        vector2.push(shifted_val2);
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
                unsafe{pc = *(area.offset(16 as isize) as *mut i64x2);}
                dmb();
                unsafe{it = *(area.offset(18 as isize) as *mut i64x2);}
// ---------------------------------------------------------------------------------- got data
                dmb();
                //------------------------------------------------------------- first 4 operators
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
                i = i + 4;

// ---------------------------------------------------------------------------------- get the data
                unsafe{pc = *(area.offset(20 as isize) as *mut i64x2);}
                dmb();
                unsafe{it = *(area.offset(22 as isize) as *mut i64x2);}
// ---------------------------------------------------------------------------------- got data
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
                i = i + 4;

// ---------------------------------------------------------------------------------- get the data
                unsafe{pc = *(area.offset(24 as isize) as *mut i64x2);}
                dmb();
                unsafe{it = *(area.offset(26 as isize) as *mut i64x2);}
// ---------------------------------------------------------------------------------- got data
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
                i = i + 4;

// ---------------------------------------------------------------------------------- get the data
                unsafe{pc = *(area.offset(28 as isize) as *mut i64x2);}
                dmb();
                unsafe{it = *(area.offset(30 as isize) as *mut i64x2);}
// ---------------------------------------------------------------------------------- got data
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
                i = 0;
                dmb();
                //println!("DONE 4");

// ---------------------------------------------------------------------------------- get the data
                unsafe{pc = *(area.offset(32 as isize) as *mut i64x2);}
                dmb();
                unsafe{it = *(area.offset(34 as isize) as *mut i64x2);}
// ---------------------------------------------------------------------------------- got data
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
                i = i + 4;

// ---------------------------------------------------------------------------------- get the data
                unsafe{pc = *(area.offset(36 as isize) as *mut i64x2);}
                dmb();
                unsafe{it = *(area.offset(38 as isize) as *mut i64x2);}
                dmb();
// ---------------------------------------------------------------------------------- got data

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
                i = i + 4;

// ---------------------------------------------------------------------------------- get the data
                unsafe{pc = *(area.offset(40 as isize) as *mut i64x2);}
                dmb();
                unsafe{it = *(area.offset(42 as isize) as *mut i64x2);}
                dmb();
// ---------------------------------------------------------------------------------- get the data

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
                i = i + 4;


// ---------------------------------------------------------------------------------- get the data
                unsafe{pc = *(area.offset(44 as isize) as *mut i64x2);}
                dmb();
                unsafe{it = *(area.offset(46 as isize) as *mut i64x2);}
                dmb();

// ---------------------------------------------------------------------------------- get the data
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
                i = 0;
                dmb();
                //println!("DONE 5");

// ---------------------------------------------------------------------------------- get the data
                unsafe{pc = *(area.offset(48 as isize) as *mut i64x2);}
                dmb();
                unsafe{it = *(area.offset(50 as isize) as *mut i64x2);}
                dmb();
// ---------------------------------------------------------------------------------- get the data

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
                unsafe{pc = *(area.offset(52 as isize) as *mut i64x2);}
                dmb();
                unsafe{it = *(area.offset(54 as isize) as *mut i64x2);}
                dmb();
// ---------------------------------------------------------------------------------- get the data

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
                i = i + 4;

// ---------------------------------------------------------------------------------- get the data
                unsafe{pc = *(area.offset(56 as isize) as *mut i64x2);}
                dmb();
                unsafe{it = *(area.offset(58 as isize) as *mut i64x2);}
                dmb();
// ---------------------------------------------------------------------------------- get the data
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
                i = i + 4;

// ---------------------------------------------------------------------------------- get the data
                unsafe{pc = *(area.offset(60 as isize) as *mut i64x2);}
                dmb();
                unsafe{it = *(area.offset(62 as isize) as *mut i64x2);}
                dmb();
// ---------------------------------------------------------------------------------- get the data

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
                i = 0;
                dmb();
                //println!("DONE 6");

// ---------------------------------------------------------------------------------- get the data
                unsafe{pc = *(area.offset(64 as isize) as *mut i64x2);}
                dmb();
                unsafe{it = *(area.offset(66 as isize) as *mut i64x2);}
                dmb();
// ---------------------------------------------------------------------------------- get the data
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
                unsafe{pc = *(area.offset(68 as isize) as *mut i64x2);}
                dmb();
                unsafe{it = *(area.offset(70 as isize) as *mut i64x2);}
                dmb();
// ---------------------------------------------------------------------------------- get the data

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
                i = i + 4;

// ---------------------------------------------------------------------------------- get the data
                unsafe{pc = *(area.offset(72 as isize) as *mut i64x2);}
                dmb();
                unsafe{it = *(area.offset(74 as isize) as *mut i64x2);}
                dmb();
// ---------------------------------------------------------------------------------- get the data

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
                i = i + 4;

// ---------------------------------------------------------------------------------- get the data
                unsafe{pc = *(area.offset(76 as isize) as *mut i64x2);}
                dmb();
                unsafe{it = *(area.offset(78 as isize) as *mut i64x2);}
                dmb();
// ---------------------------------------------------------------------------------- get the data
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
                i = 0;
                dmb();

// ---------------------------------------------------------------------------------- get the data
                unsafe{pc = *(area.offset(80 as isize) as *mut i64x2);}
                dmb();
                unsafe{it = *(area.offset(82 as isize) as *mut i64x2);}
                dmb();
// ---------------------------------------------------------------------------------- get the data

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
                i = 0;
                dmb();

// ---------------------------------------------------------------------------------- get the data
                unsafe{pc = *(area.offset(84 as isize) as *mut i64x2);}
                dmb();
                unsafe{it = *(area.offset(86 as isize) as *mut i64x2);}
                dmb();
// ---------------------------------------------------------------------------------- get the data

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
                i = 0;
                dmb();

// ---------------------------------------------------------------------------------- get the data
                unsafe{pc = *(area.offset(88 as isize) as *mut i64x2);}
                dmb();
                unsafe{it = *(area.offset(90 as isize) as *mut i64x2);}
                dmb();
// ---------------------------------------------------------------------------------- get the data

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
                i = 0;
                dmb();

// ---------------------------------------------------------------------------------- get the data
                unsafe{pc = *(area.offset(92 as isize) as *mut i64x2);}
                dmb();
                unsafe{it = *(area.offset(94 as isize) as *mut i64x2);}
                dmb();
// ---------------------------------------------------------------------------------- get the data

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
                i = 0;
                dmb();

            }

            //let epoch_end = Instant::now();
            //let total_nanos = (epoch_end - epoch_start).as_nanos();
            //println!("wrapper latency: {total_nanos}");

            if !has_data {

                let area = unsafe { (*hc).area } as *mut u64;

                let mut v1: Vec<i64x2> = Vec::new();
                let mut v0: Vec<u64x2> = Vec::new();

                let mut current_length = 0;

                let data_length = num_data;

                for i in 0 .. borrow.len() {
                    let frontier = borrow[i].borrow().frontier();
                    if frontier.len() == 0 {
                        *memory.offset(current_length as isize) = 0;
                        current_length += 1;
                    } else {
                        for val in (0..frontier.len()).step_by(2) {
                            let x =  u64x2::from_array([(frontier[val] << 1) | 1i64, (frontier[val] << 1) | 1i64]);
                            v0.push(x);
                            //current_length += 1;
                        }
                    }
                }

                for i in (current_length..max_length_in).step_by(2) {
                    let x =  u64x2::from_array([0, 0]);
                    v0.push(x);
                }

                for i in (0..16).step_by(2) {
                    let x = i64x2::from_array([data_i[i], data_i[i+1]]);
                    v1.push(x);
                }

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
                unsafe{*(area.offset(24 as isize) as *mut i64x2) = v1[12]};
                unsafe{*(area.offset(26 as isize) as *mut i64x2) = v1[13]};
                unsafe{*(area.offset(28 as isize) as *mut i64x2) = v1[14]};
                unsafe{*(area.offset(30 as isize) as *mut i64x2) = v1[15]};
                unsafe{*(area.offset(32 as isize) as *mut i64x2) = v1[16]};
                unsafe{*(area.offset(34 as isize) as *mut i64x2) = v1[17]};
                unsafe{*(area.offset(36 as isize) as *mut i64x2) = v1[18]};
                unsafe{*(area.offset(38 as isize) as *mut i64x2) = v1[19]};
                dmb();

                for i in 0..data_length as usize {
                    unsafe{pc = *(area.offset(i as isize) as *mut i64x2);}
                    // all the writes can be done asynchronously
                    // we are getting two numbers here
                    // the offset for progress would be 18
                    dmb();
                    let shifted_val1 = pc[0] >> 1;
                    let shifted_val2 = pc[1] >> 1;
                    if val1 != 0 {
                        vector2.push(shifted_val1);
                    }
                    if val2 != 0 {
                        vector2.push(shifted_val2);
                    }
                }

                let id_wrap = ghost_indexes[ghost_indexes.len() - 1].1;

                if vector2.len() > 0 {
                    output_wrapper
                        .session(&(internals.get(&id_wrap).unwrap().0 as u64))
                        .give_vec(&mut vector2);

                    let mut cb1 = ChangeBatch::new_from(
                        internals.get(&id_wrap).unwrap().0 as u64,
                        *produced.get(&id_wrap).unwrap(),
                    );
                    let mut cb2 = ChangeBatch::new_from(
                        internals.get(&id_wrap).unwrap().0 as u64,
                        internals.get(&id_wrap).unwrap().1 as i64,
                    );
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&id_wrap).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&id_wrap).unwrap()[0]);
                }

                if vector2.len() > 0 {
                    output_wrapper
                        .session(&(internals.get(&id_wrap).unwrap().0 as u64))
                        .give_vec(&mut vector2);

                    let mut k = 0;
                    let mut i = 0 as usize;
                    let mut j = 0;
                    let mut cb = ChangeBatch::new_from(0, 0);
                    let mut cb1 = ChangeBatch::new_from(0, 0);
                    let mut cb2 = ChangeBatch::new_from(0, 0);
                    let mut counter_offset = 0;

                    let time_1 = internals.get(&id_wrap).unwrap().0;

                    let mut pc: i64x2 = i64x2::from_array([0, 0]);
                    let mut it: i64x2 = i64x2::from_array([0, 0]);
// ---------------------------------------------------------------------------------- get the data
                    unsafe { pc = *(area.offset(16 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(18 as isize) as *mut i64x2); }
// ---------------------------------------------------------------------------------- got data
                    dmb();
                    //------------------------------------------------------------- first 4 operators
                    cb = ChangeBatch::new_from(time_1, pc[0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[1] as i64);
                    cb2 = ChangeBatch::new_from(
                        it[0] as u64,
                        it[1] as i64,
                    );
                    j = ghost_indexes[0].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
                    i = i + 4;

// ---------------------------------------------------------------------------------- get the data
                    unsafe { pc = *(area.offset(20 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(22 as isize) as *mut i64x2); }
// ---------------------------------------------------------------------------------- got data
                    cb = ChangeBatch::new_from(time_1, pc[0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[1] as i64);
                    cb2 = ChangeBatch::new_from(
                        it[0] as u64,
                        it[1] as i64,
                    );
                    j = ghost_indexes[1].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
                    i = i + 4;

// ---------------------------------------------------------------------------------- get the data
                    unsafe { pc = *(area.offset(24 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(26 as isize) as *mut i64x2); }
// ---------------------------------------------------------------------------------- got data
                    cb = ChangeBatch::new_from(time_1, pc[0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[1] as i64);
                    cb2 = ChangeBatch::new_from(
                        it[0] as u64,
                        it[1] as i64,
                    );

                    j = ghost_indexes[2].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
                    i = i + 4;

// ---------------------------------------------------------------------------------- get the data
                    unsafe { pc = *(area.offset(28 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(30 as isize) as *mut i64x2); }
// ---------------------------------------------------------------------------------- got data
                    cb = ChangeBatch::new_from(time_1, pc[0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[1] as i64);
                    cb2 = ChangeBatch::new_from(
                        it[0] as u64,
                        it[1] as i64,
                    );
                    j = ghost_indexes[3].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
                    i = 0;
                    dmb();
                    //println!("DONE 4");

// ---------------------------------------------------------------------------------- get the data
                    unsafe { pc = *(area.offset(32 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(34 as isize) as *mut i64x2); }
// ---------------------------------------------------------------------------------- got data
                    cb = ChangeBatch::new_from(time_1, pc[0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[1] as i64);
                    cb2 = ChangeBatch::new_from(
                        it[0] as u64,
                        it[1] as i64,
                    );
                    j = ghost_indexes[4].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
                    i = i + 4;

// ---------------------------------------------------------------------------------- get the data
                    unsafe { pc = *(area.offset(36 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(38 as isize) as *mut i64x2); }
                    dmb();
// ---------------------------------------------------------------------------------- got data

                    cb = ChangeBatch::new_from(time_1, pc[0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[1] as i64);
                    cb2 = ChangeBatch::new_from(
                        it[0] as u64,
                        it[1] as i64,
                    );
                    j = ghost_indexes[5].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
                    i = i + 4;

// ---------------------------------------------------------------------------------- get the data
                    unsafe { pc = *(area.offset(40 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(42 as isize) as *mut i64x2); }
                    dmb();
// ---------------------------------------------------------------------------------- get the data

                    cb = ChangeBatch::new_from(time_1, pc[0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[1] as i64);
                    cb2 = ChangeBatch::new_from(
                        it[0] as u64,
                        it[1] as i64,
                    );

                    j = ghost_indexes[6].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
                    i = i + 4;


// ---------------------------------------------------------------------------------- get the data
                    unsafe { pc = *(area.offset(44 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(46 as isize) as *mut i64x2); }
                    dmb();

// ---------------------------------------------------------------------------------- get the data
                    cb = ChangeBatch::new_from(time_1, pc[0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[1] as i64);
                    cb2 = ChangeBatch::new_from(
                        it[0] as u64,
                        it[1] as i64,
                    );
                    j = ghost_indexes[7].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
                    i = 0;
                    dmb();
                    //println!("DONE 5");

// ---------------------------------------------------------------------------------- get the data
                    unsafe { pc = *(area.offset(48 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(50 as isize) as *mut i64x2); }
                    dmb();
// ---------------------------------------------------------------------------------- get the data

                    cb = ChangeBatch::new_from(time_1, pc[0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[1] as i64);
                    cb2 = ChangeBatch::new_from(
                        it[0] as u64,
                        it[1] as i64,
                    );
                    j = ghost_indexes[8].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
                    i = i + 4;

// ---------------------------------------------------------------------------------- get the data
                    unsafe { pc = *(area.offset(52 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(54 as isize) as *mut i64x2); }
                    dmb();
// ---------------------------------------------------------------------------------- get the data

                    cb = ChangeBatch::new_from(time_1, pc[0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[1] as i64);
                    cb2 = ChangeBatch::new_from(
                        it[0] as u64,
                        it[1] as i64,
                    );
                    j = ghost_indexes[9].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
                    i = i + 4;

// ---------------------------------------------------------------------------------- get the data
                    unsafe { pc = *(area.offset(56 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(58 as isize) as *mut i64x2); }
                    dmb();
// ---------------------------------------------------------------------------------- get the data
                    cb = ChangeBatch::new_from(time_1, pc[0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[1] as i64);
                    cb2 = ChangeBatch::new_from(
                        it[0] as u64,
                        it[1] as i64,
                    );

                    j = ghost_indexes[10].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
                    i = i + 4;

// ---------------------------------------------------------------------------------- get the data
                    unsafe { pc = *(area.offset(60 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(62 as isize) as *mut i64x2); }
                    dmb();
// ---------------------------------------------------------------------------------- get the data

                    cb = ChangeBatch::new_from(time_1, pc[0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[1] as i64);
                    cb2 = ChangeBatch::new_from(
                        it[0] as u64,
                        it[1] as i64,
                    );
                    j = ghost_indexes[11].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
                    i = 0;
                    dmb();
                    //println!("DONE 6");

// ---------------------------------------------------------------------------------- get the data
                    unsafe { pc = *(area.offset(64 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(66 as isize) as *mut i64x2); }
                    dmb();
// ---------------------------------------------------------------------------------- get the data
                    cb = ChangeBatch::new_from(time_1, pc[0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[1] as i64);
                    cb2 = ChangeBatch::new_from(
                        it[0] as u64,
                        it[1] as i64,
                    );
                    j = ghost_indexes[12].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
                    i = i + 4;

// ---------------------------------------------------------------------------------- get the data
                    unsafe { pc = *(area.offset(68 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(70 as isize) as *mut i64x2); }
                    dmb();
// ---------------------------------------------------------------------------------- get the data

                    cb = ChangeBatch::new_from(time_1, pc[0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[1] as i64);
                    cb2 = ChangeBatch::new_from(
                        it[0] as u64,
                        it[1] as i64,
                    );
                    j = ghost_indexes[13].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
                    i = i + 4;

// ---------------------------------------------------------------------------------- get the data
                    unsafe { pc = *(area.offset(72 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(74 as isize) as *mut i64x2); }
                    dmb();
// ---------------------------------------------------------------------------------- get the data

                    cb = ChangeBatch::new_from(time_1, pc[0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[1] as i64);
                    cb2 = ChangeBatch::new_from(
                        it[0] as u64,
                        it[1] as i64,
                    );
                    j = ghost_indexes[14].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
                    i = i + 4;

// ---------------------------------------------------------------------------------- get the data
                    unsafe { pc = *(area.offset(76 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(78 as isize) as *mut i64x2); }
                    dmb();
// ---------------------------------------------------------------------------------- get the data
                    cb = ChangeBatch::new_from(time_1, pc[0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[1] as i64);
                    cb2 = ChangeBatch::new_from(
                        it[0] as u64,
                        it[1] as i64,
                    );
                    j = ghost_indexes[15].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
                    i = 0;
                    dmb();

// ---------------------------------------------------------------------------------- get the data
                    unsafe { pc = *(area.offset(80 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(82 as isize) as *mut i64x2); }
                    dmb();
// ---------------------------------------------------------------------------------- get the data

                    cb = ChangeBatch::new_from(time_1, pc[0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[1] as i64);
                    cb2 = ChangeBatch::new_from(
                        it[0] as u64,
                        it[1] as i64,
                    );
                    j = ghost_indexes[16].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
                    i = 0;
                    dmb();

// ---------------------------------------------------------------------------------- get the data
                    unsafe { pc = *(area.offset(84 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(86 as isize) as *mut i64x2); }
                    dmb();
// ---------------------------------------------------------------------------------- get the data

                    cb = ChangeBatch::new_from(time_1, pc[0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[1] as i64);
                    cb2 = ChangeBatch::new_from(
                        it[0] as u64,
                        it[1] as i64,
                    );
                    j = ghost_indexes[17].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
                    i = 0;
                    dmb();

// ---------------------------------------------------------------------------------- get the data
                    unsafe { pc = *(area.offset(88 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(90 as isize) as *mut i64x2); }
                    dmb();
// ---------------------------------------------------------------------------------- get the data

                    cb = ChangeBatch::new_from(time_1, pc[0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[1] as i64);
                    cb2 = ChangeBatch::new_from(
                        it[0] as u64,
                        it[1] as i64,
                    );
                    j = ghost_indexes[18].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
                    i = 0;
                    dmb();

// ---------------------------------------------------------------------------------- get the data
                    unsafe { pc = *(area.offset(92 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(94 as isize) as *mut i64x2); }
                    dmb();
// ---------------------------------------------------------------------------------- get the data

                    cb = ChangeBatch::new_from(time_1, pc[0] as i64);
                    cb1 = ChangeBatch::new_from(time_1, pc[1] as i64);
                    cb2 = ChangeBatch::new_from(
                        it[0] as u64,
                        it[1] as i64,
                    );
                    j = ghost_indexes[19].1 as usize;
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
                    i = 0;
                    dmb();
                }


            }

            //let epoch_end = Instant::now();
            // let total_nanos = (epoch_end - epoch_start).as_nanos();
            // println!("wrapper latency latency: {total_nanos}");

            vector.clear();
            vector2.clear();
            produced.clear();
            consumed.clear();
            internals.clear();
            output_wrapper.cease();

            //let epoch_end = Instant::now();
            //let total_nanos = (epoch_end - epoch_start).as_nanos();
            //println!("wrapper latency latency: {total_nanos}");

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
