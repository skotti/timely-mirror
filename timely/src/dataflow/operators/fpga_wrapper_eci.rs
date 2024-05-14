//! Funtionality to run operators on FPGA
pub extern crate libc;
use std::cell::{RefMut};

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

#[derive(Debug)]
#[repr(C)]
/// Data structure to store FPGA related data
pub struct HardwareCommon {
    /// the mmapped cache lines
    pub area: *mut c_void,
}

unsafe impl Send for HardwareCommon {}
unsafe impl Sync for HardwareCommon {}

//input_arr: [u64; MAX_LENGTH_IN]
//output_arr: [u64; MAX_LENGTH_OUT]

static mut GLOBAL_COUNTER: i32 = 0;


/// Sends data to FPGA and receives response
/*fn run(hc: *const HardwareCommon, num_data: i64, num_operators: i64, h_mem_arr: &mut Vec<u64>) -> Vec<u64> {
    // Only run when `no-fpga` feature is used

    let mut frontier_length = 16;//(num_operators / CACHE_LINE_SIZE) + CACHE_LINE_SIZE;
    let mut progress_length = 64;//((num_operators * 4) / CACHE_LINE_SIZE) + CACHE_LINE_SIZE;

    #[cfg(feature = "no-fpga")]
        let output_arr = generate_fpga_output(h_mem_arr, num_data, num_operators);

    // Only run when using FPGA
    #[cfg(not(feature = "no-fpga"))]
        let output_arr = {
        let frontiers: &[u64] = &h_mem_arr[0..frontier_length as usize];
        let data: &[u64] = &h_mem_arr[frontier_length as usize..(frontier_length + num_data) as usize];
        fpga_communication(hc, frontiers, data, num_data, num_operators)
    };

    output_arr
}*/

/// Writes a specific hardcoded bit pattern to simulate FPGA output
fn generate_fpga_output(input_arr: &mut Vec<u64>, num_data: i64, num_operators: i64) -> Vec<u64> {

    println!("INPUT ARRAY");
    for i in 0..input_arr.len() {
        print!("{} ", input_arr[i]);
    }
    println!();

    // Cast input buffer ptr to array
    let mut offset = 0; // Keep track while iterate through array
    let operator_count = num_operators as usize;
    let mut frontier_length = ((num_operators / CACHE_LINE_SIZE) + CACHE_LINE_SIZE) as usize;
    println!("Frontier length = {}", frontier_length);
    let mut progress_length = (((num_operators * 4) / CACHE_LINE_SIZE) + CACHE_LINE_SIZE) as usize;
    println!("Progress length = {}", progress_length);
    let max_length_in = num_data as usize + frontier_length;
    let max_length_out = num_data as usize + progress_length;
    println!("Max length in = {}", max_length_in);
    println!("Max length out = {}", max_length_out);
    //
    let same_value = input_arr[offset];
    for i in 0..operator_count {
        let i = i + offset;
        assert!(0 == input_arr[i] || input_arr[i] % 2 == 1);
        assert_eq!(same_value, input_arr[i]); // values should be the same across
    }
    offset += operator_count;

    // Safety check, otherwise we overwrite values
    assert!(offset <= frontier_length);


    let mut valid_inputs = 0;
    let mut unfiltered_inputs = 0;
    for i in 0..num_data as usize{
        let i = i + frontier_length;

        // Check if input is valid
        if input_arr[i] % 2 == 1 {
            valid_inputs += 1;
            // Check if input filtered
            if input_arr[i] >= (5 << 1 | 1) {
                unfiltered_inputs += 1;
            }
        }
        assert!(0 == input_arr[i] || input_arr[i] % 2 == 1);
    }

    //
    // Cast buffer ptr to array
    let mut output_arr = vec![0; max_length_out];
    let mut my_offset = 0;
    // 1...1 - number of inputs times
    for i in 0..num_data as usize {
        output_arr[i] = input_arr[i + frontier_length];
    }
    my_offset += num_data as usize;

    // 1100 - operator many times
    for _i in 0..num_operators {
        output_arr[my_offset + 0] = valid_inputs;
        output_arr[my_offset + 1] = unfiltered_inputs;
        output_arr[my_offset + 2] = 0;
        output_arr[my_offset + 3] = 0;

        my_offset += 4;
    }

    println!("OUTPUT ARRAY");
    for i in 0..output_arr.len() {
        print!("{} ", output_arr[i]);
    }
    println!();

    output_arr
}

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

#[cfg(feature = "32op")]
fn get_offset(offset_1: &mut i64, offset_2: &mut i64) {
    //unsafe {
    //println!("Original value: {}", GLOBAL_COUNTER);
    //if (GLOBAL_COUNTER % 2 == 0) {
    *offset_1 = 0;
    *offset_2 = CACHE_LINE_SIZE;
    //} else {
    //    *offset_1 = CACHE_LINE_SIZE;
    //    *offset_2 = 0;
    //}
    // GLOBAL_COUNTER = GLOBAL_COUNTER + 1;
    //println!("Modified value: {}", GLOBAL_COUNTER);
    //}
}


#[cfg(feature = "20op")]
fn get_offset(offset_1: &mut i64, offset_2: &mut i64) {
    unsafe {
        //println!("Original value: {}", GLOBAL_COUNTER);
        if (GLOBAL_COUNTER % 2 == 0) {
            *offset_1 = 0;
            *offset_2 = CACHE_LINE_SIZE;
        } else {
            *offset_1 = CACHE_LINE_SIZE;
            *offset_2 = 0;
        }
        GLOBAL_COUNTER = GLOBAL_COUNTER + 1;
        //println!("Modified value: {}", GLOBAL_COUNTER);
    }
}


#[cfg(feature = "16op")]
fn get_offset(offset_1: &mut i64, offset_2: &mut i64) {
    unsafe {
        //println!("Original value: {}", GLOBAL_COUNTER);
        if (GLOBAL_COUNTER % 2 == 0) {
            *offset_1 = 0;
            *offset_2 = CACHE_LINE_SIZE;
        } else {
            *offset_1 = CACHE_LINE_SIZE;
            *offset_2 = 0;
        }
        GLOBAL_COUNTER = GLOBAL_COUNTER + 1;
        //println!("Modified value: {}", GLOBAL_COUNTER);
    }
}

#[cfg(feature = "15op")]
fn get_offset(offset_1: &mut i64, offset_2: &mut i64) {
    unsafe {
        //println!("Original value: {}", GLOBAL_COUNTER);
        if (GLOBAL_COUNTER % 2 == 0) {
            *offset_1 = 0;
            *offset_2 = CACHE_LINE_SIZE;
        } else {
            *offset_1 = CACHE_LINE_SIZE;
            *offset_2 = 0;
        }
        GLOBAL_COUNTER = GLOBAL_COUNTER + 1;
        //println!("Modified value: {}", GLOBAL_COUNTER);
    }
}

#[cfg(feature = "4op")]
fn get_offset(offset_1: &mut i64, offset_2: &mut i64) {
    //unsafe {
        //println!("Original value: {}", GLOBAL_COUNTER);
    //    if (GLOBAL_COUNTER % 2 == 0) {
            *offset_1 = 0;
            *offset_2 = CACHE_LINE_SIZE;
    //    } else {
    //        *offset_1 = CACHE_LINE_SIZE;
    //        *offset_2 = 0;
    //    }
    //    GLOBAL_COUNTER = GLOBAL_COUNTER + 1;
        //println!("Modified value: {}", GLOBAL_COUNTER);
    //}
}

#[cfg(feature = "1op")]
fn get_offset(offset_1: &mut i64, offset_2: &mut i64) {
    *offset_1 = 0;
    *offset_2 = CACHE_LINE_SIZE;
}

#[cfg(feature = "1op")]
fn write_data(
    borrow: &RefMut<Vec<MutableAntichain<u64>>>,
    vector: &mut Vec<u64>,
    hc: *const HardwareCommon,
    cache_line_1: & mut[u64],
    cache_line_2: & mut[u64]
)
{
    let mut offset_1 = 0;
    let mut offset_2 = 0;

    get_offset(&mut offset_1, &mut offset_2);
    //println!("offset1 = {}, offset2 = {}", offset_1, offset_2);


    let area = unsafe { (*hc).area } as *mut u64;
    let cache_line_1 = unsafe { std::slice::from_raw_parts_mut(area.offset(offset_1.try_into().unwrap()), CACHE_LINE_SIZE as usize) };
    let cache_line_2 = unsafe {
        std::slice::from_raw_parts_mut(
            area.offset(offset_2.try_into().unwrap()),
            CACHE_LINE_SIZE as usize,
        )
    };


    let mut current_length = 0;
    for i in 0..16 {
        let frontier = borrow[i].frontier();
        if frontier.len() == 0 {
            cache_line_1[current_length] = 0;
        } else {
            //for val in frontier.iter() {
            cache_line_1[i] = (frontier[0] << 1) | 1u64;
            //}
        }
    }
    dmb();

    if vector.len() == 0 {
        for i in 0..16 {
            cache_line_2[i] = 0;
        }
    } else {
        for i in 0..16 {
            cache_line_2[i] = (vector[i] << 1) | 1u64;
            //current_length += 1;
        }
    }
    dmb();
}

#[cfg(feature = "1op")]
fn read_data(
    progress: &mut SharedProgress<u64>,
    time: &u64,
    hc: *const HardwareCommon,
    ghost_indexes: &Vec<(usize, usize)>,
    vector2: &mut Vec<u64>,
    cache_line_1: & mut[u64],
    cache_line_2: & mut[u64]
)
{

    for i in 0..16 as usize{
        let val = cache_line_1[i] as u64;
        let shifted_val = val >> 1;
        if val != 0 {
            vector2.push(shifted_val);
        }
    }

    dmb();

    let mut k = 0;
    let mut i = 0 as usize;
    let mut j = 0;
    let mut cb = ChangeBatch::new_from(0, 0);
    let mut cb1 = ChangeBatch::new_from(0, 0);
    let mut cb2 = ChangeBatch::new_from(0, 0);

    let time_1 = time.clone();

    //------------------------------------------------------------- first 4 operators
    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );
    j = ghost_indexes[0].1 as usize;
    cb.drain_into(  &mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;

    dmb();
}

#[cfg(feature = "4op")]
fn write_data(
    borrow: &RefMut<Vec<MutableAntichain<u64>>>,
    vector: &mut Vec<u64>,
    hc: *const HardwareCommon,
    cache_line_1: & mut[u64],
    cache_line_2: & mut[u64]
)
{

    /*println!("DATA TO FPGA");
    for val in vector.iter() {
       print!("{} ", val);
    }
    println!();*/

    let mut current_length = 0;

    for i in 0..4 {
        let frontier = borrow[i].frontier();
        if frontier.len() == 0 {
            cache_line_1[current_length] = 0;
        } else {
            //for val in frontier.iter() {
            cache_line_1[i] = (frontier[0] << 1) | 1u64;
            //}
        }
    }

    for i in 4..16 {
        cache_line_1[i] = 0;
    }


    dmb();

    current_length = 0;

    if vector.len() == 0 {
        for i in 0..16 {
            cache_line_2[i] = 0;
        }
    } else {
        for i in 0..16 {
            cache_line_2[i] = (vector[i] << 1) | 1u64;
            //current_length += 1;
        }
    }

    dmb();
}

#[cfg(feature = "4op")]
fn read_data(
    progress: &mut SharedProgress<u64>,
    time: &u64,
    hc: *const HardwareCommon,
    ghost_indexes: &Vec<(usize, usize)>,
    vector2: &mut Vec<u64>,
    cache_line_1: & mut[u64],
    cache_line_2: & mut[u64]
)
{
    for i in 0..16 as usize{
        let val = cache_line_1[i] as u64;
        let shifted_val = val >> 1;
        if val != 0 {
            vector2.push(shifted_val);
        }
    }

    dmb();

    /*println!("DATA FROM FPGA");
    for val in vector2.iter() {
       print!("{} ", val);
    }
    println!();*/

    let mut k = 0;
    let mut i = 0 as usize;
    let mut j = 0;
    let mut cb = ChangeBatch::new_from(0, 0);
    let mut cb1 = ChangeBatch::new_from(0, 0);
    let mut cb2 = ChangeBatch::new_from(0, 0);

    let time_1 = time.clone();

    //------------------------------------------------------------- first 4 operators
    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );
    j = ghost_indexes[0].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;

    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );
    j = ghost_indexes[1].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );

    j = ghost_indexes[2].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );
    j = ghost_indexes[3].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = 0;
    dmb();
    //println!("DONE 4");
}

#[cfg(feature = "16op")]
fn write_data(
    borrow: &RefMut<Vec<MutableAntichain<u64>>>,
    vector: &mut Vec<u64>,
    hc: *const HardwareCommon,
    cache_line_1: & mut[u64],
    cache_line_2: & mut[u64]
)
{
    let mut current_length = 0;

   /* println!("DATA TO FPGA");
   for val in vector.iter() {
       print!("{} ", val);
   }
   println!();
*/
    for i in 0..16 {
        let frontier = borrow[i].frontier();
        if frontier.len() == 0 {
            cache_line_1[current_length] = 0;
        } else {
            //for val in frontier.iter() {
            cache_line_1[i] = (frontier[0] << 1) | 1u64;
            //}
        }
    }

    dmb();

    current_length = 0;

    if vector.len() == 0 {
        for i in 0..16 {
            cache_line_2[i] = 0;
        }
    } else {
        for i in 0..16 {
            cache_line_2[i] = (vector[i] << 1) | 1u64;
            //current_length += 1;
        }
    }

    dmb();

}
#[cfg(feature = "16op")]
fn read_data(
    progress: &mut SharedProgress<u64>,
    time: &u64,
    hc: *const HardwareCommon,
    ghost_indexes: &Vec<(usize, usize)>,
    vector2: &mut Vec<u64>,
    cache_line_1: & mut[u64],
    cache_line_2: & mut[u64]
)
{

    for i in 0..16 as usize{
        let val = cache_line_1[i] as u64;
        let shifted_val = val >> 1;
        if val != 0 {
            vector2.push(shifted_val);
        }
    }

    dmb();

   /* println!("DATA FROM FPGA");
   for val in vector2.iter() {
       print!("{} ", val);
   }
   println!();
*/
    let mut k = 0;
    let mut i = 0 as usize;
    let mut j = 0;
    let mut cb = ChangeBatch::new_from(0, 0);
    let mut cb1 = ChangeBatch::new_from(0, 0);
    let mut cb2 = ChangeBatch::new_from(0, 0);

    let time_1 = time.clone();

    //------------------------------------------------------------- first 4 operators
    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );

    //println!("Index {}, progress {} {} {} {}", 0, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);
    j = ghost_indexes[0].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;

    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );
    //println!("Index {}, progress {} {} {} {}", 1, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);

    j = ghost_indexes[1].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );

    //println!("Index {}, progress {} {} {} {}", 2, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);

    j = ghost_indexes[2].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );
    //println!("Index {}, progress {} {} {} {}", 3, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);

    j = ghost_indexes[3].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = 0;
    dmb();
    //println!("DONE 4");

    //-------------------------------------------------------------------- next 4 operators

    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );

    //println!("Index {}, progress {} {} {} {}", 4, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);

    j = ghost_indexes[4].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;

    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );
    //println!("Index {}, progress {} {} {} {}", 5, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);

    j = ghost_indexes[5].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );

    //println!("Index {}, progress {} {} {} {}", 6, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);

    j = ghost_indexes[6].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );

    //println!("Index {}, progress {} {} {} {}", 7, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);

    j = ghost_indexes[7].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = 0;
    dmb();
    //println!("DONE 5");

    //-------------------------------------------------------------------------- next 4 operators

    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );

    //println!("Index {}, progress {} {} {} {}", 8, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);

    j = ghost_indexes[8].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;

    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );

    //println!("Index {}, progress {} {} {} {}", 9, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);

    j = ghost_indexes[9].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );

    //println!("Index {}, progress {} {} {} {}", 10, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);

    j = ghost_indexes[10].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );

    //println!("Index {}, progress {} {} {} {}", 11, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);

    j = ghost_indexes[11].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = 0;
    dmb();
    //println!("DONE 6");

    //------------------------------------------------------------------------ next 4 operators

    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );

    //println!("Index {}, progress {} {} {} {}", 12, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);

    j = ghost_indexes[12].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;

    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );

    //println!("Index {}, progress {} {} {} {}", 13, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);

    j = ghost_indexes[13].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );

    //println!("Index {}, progress {} {} {} {}", 14, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);

    j = ghost_indexes[14].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );
    //println!("Index {}, progress {} {} {} {}", 15, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);

    j = ghost_indexes[15].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = 0;
    dmb();
    //println!("DONE 7");

}

#[cfg(feature = "15op")]
fn write_data(
    borrow: &RefMut<Vec<MutableAntichain<u64>>>,
    vector: &mut Vec<u64>,
    hc: *const HardwareCommon,
    cache_line_1: & mut[u64],
    cache_line_2: & mut[u64]
)
{
    let mut current_length = 0;

    /* println!("DATA TO FPGA");
    for val in vector.iter() {
        print!("{} ", val);
    }
    println!();
 */
    for i in 0..15 {
        let frontier = borrow[i].frontier();
        if frontier.len() == 0 {
            cache_line_1[current_length] = 0;
        } else {
            //for val in frontier.iter() {
            cache_line_1[i] = (frontier[0] << 1) | 1u64;
            //}
        }
    }

    cache_line_1[15] = 0;

    dmb();

    current_length = 0;

    if vector.len() == 0 {
        for i in 0..16 {
            cache_line_2[i] = 0;
        }
    } else {
        for i in 0..16 {
            cache_line_2[i] = (vector[i] << 1) | 1u64;
            //current_length += 1;
        }
    }

    dmb();

}
#[cfg(feature = "15op")]
fn read_data(
    progress: &mut SharedProgress<u64>,
    time: &u64,
    hc: *const HardwareCommon,
    ghost_indexes: &Vec<(usize, usize)>,
    vector2: &mut Vec<u64>,
    cache_line_1: & mut[u64],
    cache_line_2: & mut[u64]
)
{

    for i in 0..16 as usize{
        let val = cache_line_1[i] as u64;
        let shifted_val = val >> 1;
        if val != 0 {
            vector2.push(shifted_val);
        }
    }

    dmb();

    /* println!("DATA FROM FPGA");
    for val in vector2.iter() {
        print!("{} ", val);
    }
    println!();
 */
    let mut k = 0;
    let mut i = 0 as usize;
    let mut j = 0;
    let mut cb = ChangeBatch::new_from(0, 0);
    let mut cb1 = ChangeBatch::new_from(0, 0);
    let mut cb2 = ChangeBatch::new_from(0, 0);

    let time_1 = time.clone();

    //------------------------------------------------------------- first 4 operators
    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );

    //println!("Index {}, progress {} {} {} {}", 0, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);
    j = ghost_indexes[0].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;

    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );
    //println!("Index {}, progress {} {} {} {}", 1, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);

    j = ghost_indexes[1].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );

    //println!("Index {}, progress {} {} {} {}", 2, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);

    j = ghost_indexes[2].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );
    //println!("Index {}, progress {} {} {} {}", 3, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);

    j = ghost_indexes[3].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = 0;
    dmb();
    //println!("DONE 4");

    //-------------------------------------------------------------------- next 4 operators

    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );

    //println!("Index {}, progress {} {} {} {}", 4, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);

    j = ghost_indexes[4].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;

    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );
    //println!("Index {}, progress {} {} {} {}", 5, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);

    j = ghost_indexes[5].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );

    //println!("Index {}, progress {} {} {} {}", 6, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);

    j = ghost_indexes[6].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );

    //println!("Index {}, progress {} {} {} {}", 7, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);

    j = ghost_indexes[7].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = 0;
    dmb();
    //println!("DONE 5");

    //-------------------------------------------------------------------------- next 4 operators

    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );

    //println!("Index {}, progress {} {} {} {}", 8, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);

    j = ghost_indexes[8].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;

    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );

    //println!("Index {}, progress {} {} {} {}", 9, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);

    j = ghost_indexes[9].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );

    //println!("Index {}, progress {} {} {} {}", 10, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);

    j = ghost_indexes[10].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );

    //println!("Index {}, progress {} {} {} {}", 11, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);

    j = ghost_indexes[11].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = 0;
    dmb();
    //println!("DONE 6");

    //------------------------------------------------------------------------ next 4 operators

    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );

    //println!("Index {}, progress {} {} {} {}", 12, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);

    j = ghost_indexes[12].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;

    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );

    //println!("Index {}, progress {} {} {} {}", 13, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);

    j = ghost_indexes[13].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );

    //println!("Index {}, progress {} {} {} {}", 14, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);

    j = ghost_indexes[14].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = 0;

    dmb();
    //println!("DONE 7");

}


#[cfg(feature = "20op")]
fn write_data(
    borrow: &RefMut<Vec<MutableAntichain<u64>>>,
    vector: &mut Vec<u64>,
    hc: *const HardwareCommon,
    cache_line_1: & mut[u64],
    cache_line_2: & mut[u64]
)
{

    println!("Data to FPGA");
    for val in vector.iter() {
        print!("{} ", val);
    }
    println!();

    let mut current_length = 0;

    for i in 0..16 {
        let frontier = borrow[i].frontier();
        if frontier.len() == 0 {
            cache_line_1[current_length] = 0;
        } else {
            //for val in frontier.iter() {
            cache_line_1[i] = (frontier[0] << 1) | 1u64;
            //}
        }
    }

    dmb();

    for i in 16..20 {
        let frontier = borrow[i].frontier();
        if frontier.len() == 0 {
            cache_line_2[current_length] = 0;
        } else {
            //for val in frontier.iter() {
            cache_line_2[current_length] = (frontier[0] << 1) | 1u64;
            current_length = current_length + 1;
            //}
        }
    }

    for i in 20..32 {
        cache_line_2[current_length] = 0;
        current_length = current_length + 1;
    }

    dmb();

    current_length = 0;

    if vector.len() == 0 {
        for i in 0..16 {
            cache_line_1[i] = 0;
        }
    } else {
        for i in 0..16 {
            cache_line_1[i] = (vector[i] << 1) | 1u64;
            //current_length += 1;
        }
    }
    dmb();
}
#[cfg(feature = "20op")]
fn read_data(
    progress: &mut SharedProgress<u64>,
    time: &u64,
    hc: *const HardwareCommon,
    ghost_indexes: &Vec<(usize, usize)>,
    vector2: &mut Vec<u64>,
    cache_line_1: & mut[u64],
    cache_line_2: & mut[u64]
)
{

    for i in 0..16 as usize{
        let val = cache_line_2[i] as u64;
        let shifted_val = val >> 1;
        if val != 0 {
            vector2.push(shifted_val);
        }
    }

    dmb();

    println!("Data to FPGA");
    for val in vector2.iter() {
        print!("{} ", val);
    }
    println!();

    let mut k = 0;
    let mut i = 0 as usize;
    let mut j = 0;
    let mut cb = ChangeBatch::new_from(0, 0);
    let mut cb1 = ChangeBatch::new_from(0, 0);
    let mut cb2 = ChangeBatch::new_from(0, 0);

    let time_1 = time.clone();

    //------------------------------------------------------------- first 4 operators
    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );

    println!("Index {}, progress {} {} {} {}", 0, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);
    j = ghost_indexes[0].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;

    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );

    println!("Index {}, progress {} {} {} {}", 1, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);
    j = ghost_indexes[1].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );
    println!("Index {}, progress {} {} {} {}", 2, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);

    j = ghost_indexes[2].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );

    println!("Index {}, progress {} {} {} {}", 3, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);
    j = ghost_indexes[3].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = 0;
    dmb();
    //println!("DONE 4");

    //-------------------------------------------------------------------- next 4 operators

    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );
    println!("Index {}, progress {} {} {} {}", 4, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);
    j = ghost_indexes[4].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;

    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );
    println!("Index {}, progress {} {} {} {}", 5, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);
    j = ghost_indexes[5].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );

    println!("Index {}, progress {} {} {} {}", 6, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);
    j = ghost_indexes[6].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );

    println!("Index {}, progress {} {} {} {}", 7, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);
    j = ghost_indexes[7].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = 0;
    dmb();
    //println!("DONE 5");

    //-------------------------------------------------------------------------- next 4 operators

    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );

    println!("Index {}, progress {} {} {} {}", 8, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);
    j = ghost_indexes[8].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;

    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );
    println!("Index {}, progress {} {} {} {}", 9, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);
    j = ghost_indexes[9].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );

    println!("Index {}, progress {} {} {} {}", 10, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);
    j = ghost_indexes[10].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );

    println!("Index {}, progress {} {} {} {}", 11, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);
    j = ghost_indexes[11].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = 0;
    dmb();
    //println!("DONE 6");

    //------------------------------------------------------------------------ next 4 operators

    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );

    println!("Index {}, progress {} {} {} {}", 12, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);
    j = ghost_indexes[12].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;

    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );

    println!("Index {}, progress {} {} {} {}", 13, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);
    j = ghost_indexes[13].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );

    println!("Index {}, progress {} {} {} {}", 14, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);
    j = ghost_indexes[14].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );

    println!("Index {}, progress {} {} {} {}", 15, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);
    j = ghost_indexes[15].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = 0;
    dmb();

    //------------------------------------------------------------- first 4 operators
    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );

    println!("Index {}, progress {} {} {} {}", 16, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);
    j = ghost_indexes[16].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;

    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );

    println!("Index {}, progress {} {} {} {}", 17, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);
    j = ghost_indexes[17].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );

    println!("Index {}, progress {} {} {} {}", 18, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);
    j = ghost_indexes[18].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );

    println!("Index {}, progress {} {} {} {}", 19, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);
    j = ghost_indexes[19].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = 0;
    dmb();
    //println!("DONE 4");
}

#[cfg(feature = "32op")]
fn write_data(
    borrow: &RefMut<Vec<MutableAntichain<u64>>>,
    vector: &mut Vec<u64>,
    hc: *const HardwareCommon,
    cache_line_1: & mut[u64],
    cache_line_2: & mut[u64]
)
{
    let mut current_length = 0;

    /*println!("DATA TO FPGA");
    for val in vector.iter() {
        print!("{} ", val);
    }
    println!();*/

    for i in 0..16 {
        let frontier = borrow[i].frontier();
        if frontier.len() == 0 {
            cache_line_1[current_length] = 0;
        } else {
            //for val in frontier.iter() {
            cache_line_1[i] = (frontier[0] << 1) | 1u64;
            //}
        }
    }
    dmb();

    for i in 16..32 {
        let frontier = borrow[i].frontier();
        if frontier.len() == 0 {
            cache_line_2[current_length] = 0;
        } else {
            //for val in frontier.iter() {
            cache_line_2[current_length] = (frontier[0] << 1) | 1u64;
            current_length = current_length + 1;
            //}
        }
    }

    dmb();

    current_length = 0;

    if vector.len() == 0 {
        for i in 0..16 {
            cache_line_1[i] = 0;
        }
    } else {
        for i in 0..16 {
            cache_line_1[i] = (vector[i] << 1) | 1u64;
            //current_length += 1;
        }
    }

    dmb();
    //println!("End write data");

}
#[cfg(feature = "32op")]
fn read_data(
    progress: &mut SharedProgress<u64>,
    time: &u64,
    hc: *const HardwareCommon,
    ghost_indexes: &Vec<(usize, usize)>,
    vector2: &mut Vec<u64>,
    cache_line_1: & mut[u64],
    cache_line_2: & mut[u64]
)
{

    for i in 0..16 as usize{
        let val = cache_line_2[i] as u64;
        let shifted_val = val >> 1;
        if val != 0 {
            vector2.push(shifted_val);
        }
    }

    dmb();
    /*println!("DATA FROM FPGA");
    for val in vector2.iter() {
        print!("{} ", val);
    }
    println!();
*/

    let mut k = 0;
    let mut i = 0 as usize;
    let mut j = 0;
    let mut cb = ChangeBatch::new_from(0, 0);
    let mut cb1 = ChangeBatch::new_from(0, 0);
    let mut cb2 = ChangeBatch::new_from(0, 0);

    let time_1 = time.clone();

    //------------------------------------------------------------- first 4 operators
    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );

    //println!("Index {}, progress {} {} {} {}", 0, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);
    j = ghost_indexes[0].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;

    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );

    //println!("Index {}, progress {} {} {} {}", 1, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);
    j = ghost_indexes[1].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );
    //println!("Index {}, progress {} {} {} {}", 2, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);

    j = ghost_indexes[2].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );

    //println!("Index {}, progress {} {} {} {}", 3, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);
    j = ghost_indexes[3].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = 0;
    dmb();
    //println!("DONE 4");

    //-------------------------------------------------------------------- next 4 operators

    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );
    //println!("Index {}, progress {} {} {} {}", 4, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);
    j = ghost_indexes[4].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;

    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );
    //println!("Index {}, progress {} {} {} {}", 5, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);
    j = ghost_indexes[5].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );

    //println!("Index {}, progress {} {} {} {}", 6, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);
    j = ghost_indexes[6].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );
    //println!("Index {}, progress {} {} {} {}", 7, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);
    j = ghost_indexes[7].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = 0;
    dmb();
    //println!("DONE 5");

    //-------------------------------------------------------------------------- next 4 operators

    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );
    //println!("Index {}, progress {} {} {} {}", 8, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);
    j = ghost_indexes[8].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;

    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );
    //println!("Index {}, progress {} {} {} {}", 9, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);

    j = ghost_indexes[9].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );

    //println!("Index {}, progress {} {} {} {}", 10, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);

    j = ghost_indexes[10].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );
    //println!("Index {}, progress {} {} {} {}", 11, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);

    j = ghost_indexes[11].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = 0;
    dmb();
    //println!("DONE 6");

    //------------------------------------------------------------------------ next 4 operators

    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );
    //println!("Index {}, progress {} {} {} {}", 12, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);

    j = ghost_indexes[12].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;

    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );
    //println!("Index {}, progress {} {} {} {}", 13, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);

    j = ghost_indexes[13].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );
    //println!("Index {}, progress {} {} {} {}", 14, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);

    j = ghost_indexes[14].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );
    //println!("Index {}, progress {} {} {} {}", 15, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);

    j = ghost_indexes[15].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = 0;
    dmb();

    //------------------------------------------------------------- first 4 operators
    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );
    //println!("Index {}, progress {} {} {} {}", 16, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);

    j = ghost_indexes[16].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;

    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );
    //println!("Index {}, progress {} {} {} {}", 17, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);
    j = ghost_indexes[17].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );

    //println!("Index {}, progress {} {} {} {}", 18, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);
    j = ghost_indexes[18].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );
    //println!("Index {}, progress {} {} {} {}", 19, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);
    j = ghost_indexes[19].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = 0;
    dmb();
    //println!("DONE 4");

    //-------------------------------------------------------------------- next 4 operators

    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );
    //println!("Index {}, progress {} {} {} {}", 20, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);
    j = ghost_indexes[20].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;

    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );
    //println!("Index {}, progress {} {} {} {}", 21, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);

    j = ghost_indexes[21].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );

    //println!("Index {}, progress {} {} {} {}", 22, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);

    j = ghost_indexes[22].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );
    //println!("Index {}, progress {} {} {} {}", 23, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);

    j = ghost_indexes[23].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = 0;
    dmb();
    //println!("DONE 5");

    //-------------------------------------------------------------------------- next 4 operators

    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );
    //println!("Index {}, progress {} {} {} {}", 24, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);

    j = ghost_indexes[24].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;

    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );
    //println!("Index {}, progress {} {} {} {}", 25, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);

    j = ghost_indexes[25].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );

    //println!("Index {}, progress {} {} {} {}", 26, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);

    j = ghost_indexes[26].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_1[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_1[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_1[i+2] as u64,
        cache_line_1[i+3] as i64,
    );
    //println!("Index {}, progress {} {} {} {}", 27, cache_line_1[i], cache_line_1[i+1], cache_line_1[i+2], cache_line_1[i+3]);

    j = ghost_indexes[27].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = 0;
    dmb();
    //println!("DONE 6");

    //------------------------------------------------------------------------ next 4 operators

    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );
    //println!("Index {}, progress {} {} {} {}", 28, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);

    j = ghost_indexes[28].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;

    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );
    //println!("Index {}, progress {} {} {} {}", 29, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);

    j = ghost_indexes[29].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );
    //println!("Index {}, progress {} {} {} {}", 30, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);

    j = ghost_indexes[30].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = i + 4;


    cb = ChangeBatch::new_from(time_1, cache_line_2[i] as i64 );
    cb1 = ChangeBatch::new_from(time_1, cache_line_2[i+1] as i64 );
    cb2 = ChangeBatch::new_from(
        cache_line_2[i+2] as u64,
        cache_line_2[i+3] as i64,
    );
    //println!("Index {}, progress {} {} {} {}", 31, cache_line_2[i], cache_line_2[i+1], cache_line_2[i+2], cache_line_2[i+3]);

    j = ghost_indexes[31].1 as usize;
    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(&j).unwrap()[0]);
    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(&j).unwrap()[0]);
    cb2.drain_into(&mut progress.wrapper_internals.get_mut(&j).unwrap()[0]);
    i = 0;
    dmb();

}

/// Communicates to FPGA via cache lines using [`2fast2forward`](https://gitlab.inf.ethz.ch/PROJECT-Enzian/fpga-sources/enzian-applications/2fast2forward)
#[cfg(feature = "4op")]
fn fpga_communication(
    hc: *const HardwareCommon,
    frontiers: &[u64],
    data: &[u64],
    num_data: i64,
    num_operators: i64,
    cache_line_1: &mut[u64],
    cache_line_2: &mut[u64]
) -> Vec<u64>
{

    //let mut frontier_length = (((num_operators - 1) / CACHE_LINE_SIZE) + CACHE_LINE_SIZE) as usize;
    let mut frontier_length = 16;
    //println!("Frontier length = {}", frontier_length);
    //let mut progress_length = (((num_operators * 4 - 1) / CACHE_LINE_SIZE)* CACHE_LINE_SIZE + CACHE_LINE_SIZE) as usize;
    let mut progress_length = 16;
    //println!("Progress length = {}", progress_length);
    let max_length_in = num_data as usize + frontier_length;
    let max_length_out = num_data as usize + progress_length;
    //println!("Max length in = {}", max_length_in);
    //println!("Max length out = {}", max_length_out);

    let mut output_arr= vec![0; max_length_out];

    for i in 0..CACHE_LINE_SIZE as usize {
        cache_line_1[i] = frontiers[i];
    }
    dmb();


    let num_batch_lines = num_data / CACHE_LINE_SIZE;
    for k in 0..num_batch_lines {
        // Write data to second cache line
        for i in 0..CACHE_LINE_SIZE as usize {
            cache_line_2[i] = data[i + (CACHE_LINE_SIZE * k) as usize];
        }
        //let epoch_start = Instant::now();
        dmb();

        //let epoch_start = Instant::now();
        // Read data out
        for i in 0..CACHE_LINE_SIZE as usize {
            output_arr[i + (CACHE_LINE_SIZE * k) as usize] = cache_line_1[i];
        }
        dmb();
        //let epoch_end = Instant::now();
        //let total_nanos = (epoch_end - start).as_nanos();
        //println!("processing latency: {total_nanos}");

    }

    // Read summary
    for i in 0..CACHE_LINE_SIZE as usize {
        output_arr[i + num_data as usize ] = cache_line_2[i];
    }
    dmb();

    output_arr
}

/// Communicates to FPGA via cache lines using [`2fast2forward`](https://gitlab.inf.ethz.ch/PROJECT-Enzian/fpga-sources/enzian-applications/2fast2forward)
#[cfg(feature = "16op")]
fn fpga_communication(
    hc: *const HardwareCommon,
    frontiers: &[u64],
    data: &[u64],
    num_data: i64,
    num_operators: i64,
    cache_line_1: &mut[u64],
    cache_line_2: &mut[u64]
) -> Vec<u64>
{

    //let mut frontier_length = (((num_operators - 1) / CACHE_LINE_SIZE) + CACHE_LINE_SIZE) as usize;
    let mut frontier_length = 16;
    //println!("Frontier length = {}", frontier_length);
    //let mut progress_length = (((num_operators * 4 - 1) / CACHE_LINE_SIZE)* CACHE_LINE_SIZE + CACHE_LINE_SIZE) as usize;
    let mut progress_length = 64;
    //println!("Progress length = {}", progress_length);
    let max_length_in = num_data as usize + frontier_length;
    let max_length_out = num_data as usize + progress_length;
    //println!("Max length in = {}", max_length_in);
    //println!("Max length out = {}", max_length_out);

    let mut output_arr= vec![0; max_length_out];

    for i in 0..CACHE_LINE_SIZE as usize {
        cache_line_1[i] = frontiers[i];
    }
    dmb();


    let num_batch_lines = num_data / CACHE_LINE_SIZE;
    for k in 0..num_batch_lines {
        // Write data to second cache line
        for i in 0..CACHE_LINE_SIZE as usize {
            cache_line_2[i] = data[i + (CACHE_LINE_SIZE * k) as usize];
        }
        //let epoch_start = Instant::now();
        dmb();

        //let epoch_start = Instant::now();
        // Read data out
        for i in 0..CACHE_LINE_SIZE as usize {
            output_arr[i + (CACHE_LINE_SIZE * k) as usize] = cache_line_1[i];
        }
        dmb();
        //let epoch_end = Instant::now();
        //let total_nanos = (epoch_end - start).as_nanos();
        //println!("processing latency: {total_nanos}");

    }

    // Read summary
    for i in 0..CACHE_LINE_SIZE as usize {
        output_arr[i + num_data as usize ] = cache_line_2[i];
    }
    dmb();

    for i in 0..CACHE_LINE_SIZE as usize {
        output_arr[i + CACHE_LINE_SIZE as usize + num_data as usize ] = cache_line_1[i];
    }
    dmb();

    for i in 0..CACHE_LINE_SIZE as usize {
        output_arr[i + 2 * CACHE_LINE_SIZE as usize + num_data as usize ] = cache_line_2[i];
    }
    dmb();

    for i in 0..CACHE_LINE_SIZE as usize {
        output_arr[i + 3 * CACHE_LINE_SIZE as usize + num_data as usize ] = cache_line_1[i];
    }
    dmb();

    output_arr
}

/// Communicates to FPGA via cache lines using [`2fast2forward`](https://gitlab.inf.ethz.ch/PROJECT-Enzian/fpga-sources/enzian-applications/2fast2forward)
#[cfg(feature = "15op")]
fn fpga_communication(
    hc: *const HardwareCommon,
    frontiers: &[u64],
    data: &[u64],
    num_data: i64,
    num_operators: i64,
    cache_line_1: &mut[u64],
    cache_line_2: &mut[u64]
) -> Vec<u64>
{

    //let mut frontier_length = (((num_operators - 1) / CACHE_LINE_SIZE) + CACHE_LINE_SIZE) as usize;
    let mut frontier_length = 16;
    //println!("Frontier length = {}", frontier_length);
    //let mut progress_length = (((num_operators * 4 - 1) / CACHE_LINE_SIZE)* CACHE_LINE_SIZE + CACHE_LINE_SIZE) as usize;
    let mut progress_length = 64;
    //println!("Progress length = {}", progress_length);
    let max_length_in = num_data as usize + frontier_length;
    let max_length_out = num_data as usize + progress_length;
    //println!("Max length in = {}", max_length_in);
    //println!("Max length out = {}", max_length_out);

    let mut output_arr= vec![0; max_length_out];

    for i in 0..16 as usize {
        cache_line_1[i] = frontiers[i];
    }
    dmb();


    let num_batch_lines = num_data / CACHE_LINE_SIZE;
    for k in 0..num_batch_lines {
        // Write data to second cache line
        for i in 0..CACHE_LINE_SIZE as usize {
            cache_line_2[i] = data[i + (CACHE_LINE_SIZE * k) as usize];
        }
        //let epoch_start = Instant::now();
        dmb();

        //let epoch_start = Instant::now();
        // Read data out
        for i in 0..CACHE_LINE_SIZE as usize {
            output_arr[i + (CACHE_LINE_SIZE * k) as usize] = cache_line_1[i];
        }
        dmb();
        //let epoch_end = Instant::now();
        //let total_nanos = (epoch_end - start).as_nanos();
        //println!("processing latency: {total_nanos}");

    }

    // Read summary
    for i in 0..CACHE_LINE_SIZE as usize {
        output_arr[i + num_data as usize ] = cache_line_2[i];
    }
    dmb();

    for i in 0..CACHE_LINE_SIZE as usize {
        output_arr[i + CACHE_LINE_SIZE as usize + num_data as usize ] = cache_line_1[i];
    }
    dmb();

    for i in 0..CACHE_LINE_SIZE as usize {
        output_arr[i + 2 * CACHE_LINE_SIZE as usize + num_data as usize ] = cache_line_2[i];
    }
    dmb();

    for i in 0..CACHE_LINE_SIZE as usize {
        output_arr[i + 3 * CACHE_LINE_SIZE as usize + num_data as usize ] = cache_line_1[i];
    }
    dmb();

    output_arr
}

/// Sends data to FPGA and receives response
fn run(
    hc: *const HardwareCommon,
    num_data: i64,
    num_operators: i64,
    h_mem_arr: &mut Vec<u64>,
    cache_line_1: &mut[u64],
    cache_line_2: &mut[u64]

) -> Vec<u64>
{
    // Only run when `no-fpga` feature is used

    let mut frontier_length = 16;
    let mut progress_length = 60;//((num_operators * 4) / CACHE_LINE_SIZE) + CACHE_LINE_SIZE;

    get_length(&mut frontier_length, &mut progress_length);

    #[cfg(feature = "no-fpga")]
        let output_arr = generate_fpga_output(h_mem_arr, num_data, num_operators);

    // Only run when using FPGA
    #[cfg(not(feature = "no-fpga"))]
        let output_arr = {
        let frontiers: &[u64] = &h_mem_arr[0..frontier_length as usize];
        let data: &[u64] = &h_mem_arr[frontier_length as usize..(frontier_length as i64 + num_data) as usize];
        fpga_communication(hc, frontiers, data, num_data, num_operators, cache_line_1, cache_line_2)
    };

    output_arr
}

/// Communicates to FPGA via cache lines using [`2fast2forward`](https://gitlab.inf.ethz.ch/PROJECT-Enzian/fpga-sources/enzian-applications/2fast2forward)
#[cfg(feature = "32op")]
fn fpga_communication(
    hc: *const HardwareCommon,
    frontiers: &[u64],
    data: &[u64],
    num_data: i64,
    num_operators: i64,
    cache_line_1: &mut[u64],
    cache_line_2: &mut[u64]
) -> Vec<u64>
{

    //let mut frontier_length = (((num_operators - 1) / CACHE_LINE_SIZE) + CACHE_LINE_SIZE) as usize;
    let mut frontier_length = 32;
    //println!("Frontier length = {}", frontier_length);
    //let mut progress_length = (((num_operators * 4 - 1) / CACHE_LINE_SIZE)* CACHE_LINE_SIZE + CACHE_LINE_SIZE) as usize;
    let mut progress_length = 128;
    //println!("Progress length = {}", progress_length);
    let max_length_in = num_data as usize + frontier_length;
    let max_length_out = num_data as usize + progress_length;
    //println!("Max length in = {}", max_length_in);
    //println!("Max length out = {}", max_length_out);

    let mut output_arr= vec![0; max_length_out];

    for i in 0..CACHE_LINE_SIZE as usize {
        cache_line_1[i] = frontiers[i];
    }
    dmb();

    for i in 0..CACHE_LINE_SIZE as usize {
        cache_line_2[i] = frontiers[i + 16];
    }
    dmb();

    let num_batch_lines = num_data / CACHE_LINE_SIZE;
    for k in 0..num_batch_lines {
        // Write data to second cache line
        for i in 0..CACHE_LINE_SIZE as usize {
            cache_line_1[i] = data[i + (CACHE_LINE_SIZE * k) as usize];
        }
        //let epoch_start = Instant::now();
        dmb();

        //let epoch_start = Instant::now();
        // Read data out
        for i in 0..CACHE_LINE_SIZE as usize {
            output_arr[i + (CACHE_LINE_SIZE * k) as usize] = cache_line_2[i];
        }
        dmb();
        //let epoch_end = Instant::now();
        //let total_nanos = (epoch_end - start).as_nanos();
        //println!("processing latency: {total_nanos}");

    }

    // Read summary
    for i in 0..CACHE_LINE_SIZE as usize {
        output_arr[i + num_data as usize ] = cache_line_1[i];
    }
    dmb();

    for i in 0..CACHE_LINE_SIZE as usize {
        output_arr[i + CACHE_LINE_SIZE as usize + num_data as usize ] = cache_line_2[i];
    }
    dmb();

    for i in 0..CACHE_LINE_SIZE as usize {
        output_arr[i + 2 * CACHE_LINE_SIZE as usize + num_data as usize ] = cache_line_1[i];
    }
    dmb();

    for i in 0..CACHE_LINE_SIZE as usize {
        output_arr[i + 3 * CACHE_LINE_SIZE as usize + num_data as usize ] = cache_line_2[i];
    }
    dmb();

    // Read summary
    for i in 0..CACHE_LINE_SIZE as usize {
        output_arr[i + 4 * CACHE_LINE_SIZE as usize + num_data as usize ] = cache_line_1[i];
    }
    dmb();

    for i in 0..CACHE_LINE_SIZE as usize {
        output_arr[i + 5 * CACHE_LINE_SIZE as usize + num_data as usize ] = cache_line_2[i];
    }
    dmb();

    for i in 0..CACHE_LINE_SIZE as usize {
        output_arr[i + 6 * CACHE_LINE_SIZE as usize + num_data as usize ] = cache_line_1[i];
    }
    dmb();

    for i in 0..CACHE_LINE_SIZE as usize {
        output_arr[i + 7 * CACHE_LINE_SIZE as usize + num_data as usize ] = cache_line_2[i];
    }
    dmb();

    output_arr
}

/// Communicates to FPGA via cache lines using [`2fast2forward`](https://gitlab.inf.ethz.ch/PROJECT-Enzian/fpga-sources/enzian-applications/2fast2forward)
#[cfg(feature = "20op")]
fn fpga_communication(
    hc: *const HardwareCommon,
    frontiers: &[u64],
    data: &[u64],
    num_data: i64,
    num_operators: i64,
    cache_line_1: &mut[u64],
    cache_line_2: &mut[u64]
) -> Vec<u64>
{

    //let mut frontier_length = (((num_operators - 1) / CACHE_LINE_SIZE) + CACHE_LINE_SIZE) as usize;
    let mut frontier_length = 32;
    //println!("Frontier length = {}", frontier_length);
    //let mut progress_length = (((num_operators * 4 - 1) / CACHE_LINE_SIZE)* CACHE_LINE_SIZE + CACHE_LINE_SIZE) as usize;
    let mut progress_length = 80;
    //println!("Progress length = {}", progress_length);
    let max_length_in = num_data as usize + frontier_length;
    let max_length_out = num_data as usize + progress_length;
    //println!("Max length in = {}", max_length_in);
    //println!("Max length out = {}", max_length_out);

    let mut output_arr= vec![0; max_length_out];

    for i in 0..CACHE_LINE_SIZE as usize {
        cache_line_1[i] = frontiers[i];
    }
    dmb();

    for i in 0..CACHE_LINE_SIZE as usize {
        cache_line_2[i] = frontiers[i + 16];
    }
    dmb();

    let num_batch_lines = num_data / CACHE_LINE_SIZE;
    for k in 0..num_batch_lines {
        // Write data to second cache line
        for i in 0..CACHE_LINE_SIZE as usize {
            cache_line_1[i] = data[i + (CACHE_LINE_SIZE * k) as usize];
        }
        //let epoch_start = Instant::now();
        dmb();

        //let epoch_start = Instant::now();
        // Read data out
        for i in 0..CACHE_LINE_SIZE as usize {
            output_arr[i + (CACHE_LINE_SIZE * k) as usize] = cache_line_2[i];
        }
        dmb();
        //let epoch_end = Instant::now();
        //let total_nanos = (epoch_end - start).as_nanos();
        //println!("processing latency: {total_nanos}");

    }

    // Read summary
    for i in 0..CACHE_LINE_SIZE as usize {
        output_arr[i + num_data as usize ] = cache_line_1[i];
    }
    dmb();

    for i in 0..CACHE_LINE_SIZE as usize {
        output_arr[i + CACHE_LINE_SIZE as usize + num_data as usize ] = cache_line_2[i];
    }
    dmb();

    for i in 0..CACHE_LINE_SIZE as usize {
        output_arr[i + 2 * CACHE_LINE_SIZE as usize + num_data as usize ] = cache_line_1[i];
    }
    dmb();

    for i in 0..CACHE_LINE_SIZE as usize {
        output_arr[i + 3 * CACHE_LINE_SIZE as usize + num_data as usize ] = cache_line_2[i];
    }
    dmb();

    // Read summary
    for i in 0..CACHE_LINE_SIZE as usize {
        output_arr[i + 4 * CACHE_LINE_SIZE as usize + num_data as usize ] = cache_line_1[i];
    }
    dmb();


    output_arr
}

#[cfg(feature = "32op")]
fn get_length(
    frontier_length: &mut i32,
    progress_length: &mut i32
) {
    *frontier_length = 32;
    *progress_length = 128;
}

#[cfg(feature = "16op")]
fn get_length(
    frontier_length: &mut i32,
    progress_length: &mut i32
) {
    *frontier_length = 16;
    *progress_length = 64;
}

#[cfg(feature = "4op")]
fn get_length(
    frontier_length: &mut i32,
    progress_length: &mut i32
) {
    *frontier_length = 16;
    *progress_length = 16;
}

#[cfg(feature = "20op")]
fn get_length(
    frontier_length: &mut i32,
    progress_length: &mut i32
) {
    *frontier_length = 32;
    *progress_length = 80;
}

#[cfg(feature = "15op")]
fn get_length(
    frontier_length: &mut i32,
    progress_length: &mut i32
) {
    *frontier_length = 16;
    *progress_length = 64;
}

/// Wrapper to run on FPGA
pub trait FpgaWrapperECI<S: Scope> {
    /// Wrapper function
    fn fpga_wrapper_eci(&self, num_data: i64, num_operators: i64, hc: *const HardwareCommon) -> Stream<S, u64>;
}

// return value should be the value of the last operator

impl<S: Scope<Timestamp = u64>> FpgaWrapperECI<S> for Stream<S, u64> {
    fn fpga_wrapper_eci(&self, num_data: i64, num_operators: i64, hc: *const HardwareCommon) -> Stream<S, u64> {
        // this should correspond to the way the data will be read on the fpga
        let mut ghost_indexes = Vec::new();
        let mut ghost_indexes2 = Vec::new();
        // TODO: should get rid of ghost indexes
        let mut current_index = 0;

        let mut frontier_length = 0;//(num_operators / CACHE_LINE_SIZE) + CACHE_LINE_SIZE;
        let mut progress_length = 0;//((num_operators * 4) / CACHE_LINE_SIZE) + CACHE_LINE_SIZE;

        get_length(&mut frontier_length, &mut progress_length);

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


        let raw_logic = move |progress: &mut SharedProgress<S::Timestamp>| {

            //let epoch_start = Instant::now();

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

            let mut offset_1 = 0;
            let mut offset_2 = 0;

            get_offset(&mut offset_1, &mut offset_2);
            println!("offset1 = {}, offset2 = {}", offset_1, offset_2);


            let area = unsafe { (*hc).area } as *mut u64;
            let cache_line_1 = unsafe { std::slice::from_raw_parts_mut(area.offset(offset_1.try_into().unwrap()), CACHE_LINE_SIZE as usize) };
            let cache_line_2 = unsafe {
                std::slice::from_raw_parts_mut(
                    area.offset(offset_2.try_into().unwrap()),
                    CACHE_LINE_SIZE as usize,
                )
            };

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

                write_data(&borrow, &mut vector, hc, cache_line_1, cache_line_2);

                read_data(progress, time, hc, &ghost_indexes, &mut vector2, cache_line_1, cache_line_2);

                output_wrapper.session(time).give_vec(&mut vector2);
            }

            //let epoch_end = Instant::now();
            //let total_nanos = (epoch_end - epoch_start).as_nanos();
            //println!("wrapper latency: {total_nanos}");

            if !has_data {
                let mut current_length = 0;

                let data_length = num_data;
                let mut input_memory = vec![0; max_length_in];

                for i in 0..borrow.len() {
                    let frontier = borrow[i].frontier();
                    if frontier.len() == 0 {
                        input_memory[current_length] = 0;
                        current_length += 1;
                    } else {
                        for val in frontier.iter() {
                            input_memory[current_length] = (*val << 1) | 1u64;
                            current_length += 1;
                        }
                    }
                }

                for i in current_length..max_length_in {
                    input_memory[i] = 0;
                }
                let memory_out = run(hc, num_data, num_operators, &mut input_memory, cache_line_1, cache_line_2);

                for i in 0..data_length as usize {
                    let val = memory_out[i] as u64;
                    let shifted_val = val >> 1;
                    if val != 0 {
                        vector2.push(shifted_val);
                    }
                }

                for (i, j) in ghost_indexes.iter() {
                    let consumed_value = memory_out[progress_start_index + 4 * i] as i64;
                    let produced_value = memory_out[progress_start_index + 4 * i + 1] as i64;
                    let internals_time = (memory_out[progress_start_index + 4 * i + 2] >> 1) as u64;
                    let internals_value = memory_out[progress_start_index + 4 * i + 3] as i64;

                    consumed.insert(*j, consumed_value);
                    internals.insert(*j, (internals_time, internals_value));
                    produced.insert(*j, produced_value);
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
