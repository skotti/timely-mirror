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

            let mut data_length: i64 = 1024;

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
                    if frontier1.len() == 0 {
                        let x =  u64x2::from_array([0, 0]);
                        v0.push(x);
                        current_length += 2;
                    } else {

                        // for now we will assume that frontier has length 1, if it is not 1 , then we already might want to modify logic on the
                        // FPGA side as well
                        let x =  u64x2::from_array([(frontier1[0] << 1) | 1u64, (frontier2[0] << 1) | 1u64]);
                        v0.push(x);
                        current_length += 2;
                    }
                }



                let frontier_last = borrow[30].borrow().frontier();
                if frontier_last.len() == 0 {
                    let x =  u64x2::from_array([0, 0]);
                    v0.push(x);
                } else {
                    let x =  u64x2::from_array([(frontier_last[0] << 1) | 1u64, 0]);
                    v0.push(x);
                }

                //println!("Current length = {}", current_length);

                for i in (0..1024).step_by(2) {
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
                    unsafe{*(area.offset(48 as isize) as *mut u64x2) = v0[24]};
                    unsafe{*(area.offset(50 as isize) as *mut u64x2) = v0[25]};
                    unsafe{*(area.offset(52 as isize) as *mut u64x2) = v0[26]};
                    unsafe{*(area.offset(54 as isize) as *mut u64x2) = v0[27]};
                    unsafe{*(area.offset(56 as isize) as *mut u64x2) = v0[28]};
                    unsafe{*(area.offset(58 as isize) as *mut u64x2) = v0[29]};
                    unsafe{*(area.offset(60 as isize) as *mut u64x2) = v0[30]};
                    unsafe{*(area.offset(62 as isize) as *mut u64x2) = v0[31]};
                    unsafe{*(area.offset(64 as isize) as *mut u64x2) = v0[32]};
                    unsafe{*(area.offset(66 as isize) as *mut u64x2) = v0[33]};
                    unsafe{*(area.offset(68 as isize) as *mut u64x2) = v0[34]};
                    unsafe{*(area.offset(70 as isize) as *mut u64x2) = v0[35]};
                    unsafe{*(area.offset(72 as isize) as *mut u64x2) = v0[36]};
                    unsafe{*(area.offset(74 as isize) as *mut u64x2) = v0[37]};
                    unsafe{*(area.offset(76 as isize) as *mut u64x2) = v0[38]};
                    unsafe{*(area.offset(78 as isize) as *mut u64x2) = v0[39]};
                    unsafe{*(area.offset(80 as isize) as *mut u64x2) = v0[40]};
                    unsafe{*(area.offset(82 as isize) as *mut u64x2) = v0[41]};
                    unsafe{*(area.offset(84 as isize) as *mut u64x2) = v0[42]};
                    unsafe{*(area.offset(86 as isize) as *mut u64x2) = v0[43]};
                    unsafe{*(area.offset(88 as isize) as *mut u64x2) = v0[44]};
                    unsafe{*(area.offset(90 as isize) as *mut u64x2) = v0[45]};
                    unsafe{*(area.offset(92 as isize) as *mut u64x2) = v0[46]};
                    unsafe{*(area.offset(94 as isize) as *mut u64x2) = v0[47]};
                    unsafe{*(area.offset(96 as isize) as *mut u64x2) = v0[48]};
                    unsafe{*(area.offset(98 as isize) as *mut u64x2) = v0[49]};
                    unsafe{*(area.offset(100 as isize) as *mut u64x2) = v0[50]};
                    unsafe{*(area.offset(102 as isize) as *mut u64x2) = v0[51]};
                    unsafe{*(area.offset(104 as isize) as *mut u64x2) = v0[52]};
                    unsafe{*(area.offset(106 as isize) as *mut u64x2) = v0[53]};
                    unsafe{*(area.offset(108 as isize) as *mut u64x2) = v0[54]};
                    unsafe{*(area.offset(110 as isize) as *mut u64x2) = v0[55]};
                    unsafe{*(area.offset(112 as isize) as *mut u64x2) = v0[56]};
                    unsafe{*(area.offset(114 as isize) as *mut u64x2) = v0[57]};
                    unsafe{*(area.offset(116 as isize) as *mut u64x2) = v0[58]};
                    unsafe{*(area.offset(118 as isize) as *mut u64x2) = v0[59]};
                    unsafe{*(area.offset(120 as isize) as *mut u64x2) = v0[60]};
                    unsafe{*(area.offset(122 as isize) as *mut u64x2) = v0[61]};
                    unsafe{*(area.offset(124 as isize) as *mut u64x2) = v0[62]};
                    unsafe{*(area.offset(126 as isize) as *mut u64x2) = v0[63]};
                    unsafe{*(area.offset(128 as isize) as *mut u64x2) = v0[64]};
                    unsafe{*(area.offset(130 as isize) as *mut u64x2) = v0[65]};
                    unsafe{*(area.offset(132 as isize) as *mut u64x2) = v0[66]};
                    unsafe{*(area.offset(134 as isize) as *mut u64x2) = v0[67]};
                    unsafe{*(area.offset(136 as isize) as *mut u64x2) = v0[68]};
                    unsafe{*(area.offset(138 as isize) as *mut u64x2) = v0[69]};
                    unsafe{*(area.offset(140 as isize) as *mut u64x2) = v0[70]};
                    unsafe{*(area.offset(142 as isize) as *mut u64x2) = v0[71]};
                    unsafe{*(area.offset(144 as isize) as *mut u64x2) = v0[72]};
                    unsafe{*(area.offset(146 as isize) as *mut u64x2) = v0[73]};
                    unsafe{*(area.offset(148 as isize) as *mut u64x2) = v0[74]};
                    unsafe{*(area.offset(150 as isize) as *mut u64x2) = v0[75]};
                    unsafe{*(area.offset(152 as isize) as *mut u64x2) = v0[76]};
                    unsafe{*(area.offset(154 as isize) as *mut u64x2) = v0[77]};
                    unsafe{*(area.offset(156 as isize) as *mut u64x2) = v0[78]};
                    unsafe{*(area.offset(158 as isize) as *mut u64x2) = v0[79]};
                    unsafe{*(area.offset(160 as isize) as *mut u64x2) = v0[80]};
                    unsafe{*(area.offset(162 as isize) as *mut u64x2) = v0[81]};
                    unsafe{*(area.offset(164 as isize) as *mut u64x2) = v0[82]};
                    unsafe{*(area.offset(166 as isize) as *mut u64x2) = v0[83]};
                    unsafe{*(area.offset(168 as isize) as *mut u64x2) = v0[84]};
                    unsafe{*(area.offset(170 as isize) as *mut u64x2) = v0[85]};
                    unsafe{*(area.offset(172 as isize) as *mut u64x2) = v0[86]};
                    unsafe{*(area.offset(174 as isize) as *mut u64x2) = v0[87]};
                    unsafe{*(area.offset(176 as isize) as *mut u64x2) = v0[88]};
                    unsafe{*(area.offset(178 as isize) as *mut u64x2) = v0[89]};
                    unsafe{*(area.offset(180 as isize) as *mut u64x2) = v0[90]};
                    unsafe{*(area.offset(182 as isize) as *mut u64x2) = v0[91]};
                    unsafe{*(area.offset(184 as isize) as *mut u64x2) = v0[92]};
                    unsafe{*(area.offset(186 as isize) as *mut u64x2) = v0[93]};
                    unsafe{*(area.offset(188 as isize) as *mut u64x2) = v0[94]};
                    unsafe{*(area.offset(190 as isize) as *mut u64x2) = v0[95]};
                    unsafe{*(area.offset(192 as isize) as *mut u64x2) = v0[96]};
                    unsafe{*(area.offset(194 as isize) as *mut u64x2) = v0[97]};
                    unsafe{*(area.offset(196 as isize) as *mut u64x2) = v0[98]};
                    unsafe{*(area.offset(198 as isize) as *mut u64x2) = v0[99]};
                    unsafe{*(area.offset(200 as isize) as *mut u64x2) = v0[100]};
                    unsafe{*(area.offset(202 as isize) as *mut u64x2) = v0[101]};
                    unsafe{*(area.offset(204 as isize) as *mut u64x2) = v0[102]};
                    unsafe{*(area.offset(206 as isize) as *mut u64x2) = v0[103]};
                    unsafe{*(area.offset(208 as isize) as *mut u64x2) = v0[104]};
                    unsafe{*(area.offset(210 as isize) as *mut u64x2) = v0[105]};
                    unsafe{*(area.offset(212 as isize) as *mut u64x2) = v0[106]};
                    unsafe{*(area.offset(214 as isize) as *mut u64x2) = v0[107]};
                    unsafe{*(area.offset(216 as isize) as *mut u64x2) = v0[108]};
                    unsafe{*(area.offset(218 as isize) as *mut u64x2) = v0[109]};
                    unsafe{*(area.offset(220 as isize) as *mut u64x2) = v0[110]};
                    unsafe{*(area.offset(222 as isize) as *mut u64x2) = v0[111]};
                    unsafe{*(area.offset(224 as isize) as *mut u64x2) = v0[112]};
                    unsafe{*(area.offset(226 as isize) as *mut u64x2) = v0[113]};
                    unsafe{*(area.offset(228 as isize) as *mut u64x2) = v0[114]};
                    unsafe{*(area.offset(230 as isize) as *mut u64x2) = v0[115]};
                    unsafe{*(area.offset(232 as isize) as *mut u64x2) = v0[116]};
                    unsafe{*(area.offset(234 as isize) as *mut u64x2) = v0[117]};
                    unsafe{*(area.offset(236 as isize) as *mut u64x2) = v0[118]};
                    unsafe{*(area.offset(238 as isize) as *mut u64x2) = v0[119]};
                    unsafe{*(area.offset(240 as isize) as *mut u64x2) = v0[120]};
                    unsafe{*(area.offset(242 as isize) as *mut u64x2) = v0[121]};
                    unsafe{*(area.offset(244 as isize) as *mut u64x2) = v0[122]};
                    unsafe{*(area.offset(246 as isize) as *mut u64x2) = v0[123]};
                    unsafe{*(area.offset(248 as isize) as *mut u64x2) = v0[124]};
                    unsafe{*(area.offset(250 as isize) as *mut u64x2) = v0[125]};
                    unsafe{*(area.offset(252 as isize) as *mut u64x2) = v0[126]};
                    unsafe{*(area.offset(254 as isize) as *mut u64x2) = v0[127]};
                    unsafe{*(area.offset(256 as isize) as *mut u64x2) = v0[128]};
                    unsafe{*(area.offset(258 as isize) as *mut u64x2) = v0[129]};
                    unsafe{*(area.offset(260 as isize) as *mut u64x2) = v0[130]};
                    unsafe{*(area.offset(262 as isize) as *mut u64x2) = v0[131]};
                    unsafe{*(area.offset(264 as isize) as *mut u64x2) = v0[132]};
                    unsafe{*(area.offset(266 as isize) as *mut u64x2) = v0[133]};
                    unsafe{*(area.offset(268 as isize) as *mut u64x2) = v0[134]};
                    unsafe{*(area.offset(270 as isize) as *mut u64x2) = v0[135]};
                    unsafe{*(area.offset(272 as isize) as *mut u64x2) = v0[136]};
                    unsafe{*(area.offset(274 as isize) as *mut u64x2) = v0[137]};
                    unsafe{*(area.offset(276 as isize) as *mut u64x2) = v0[138]};
                    unsafe{*(area.offset(278 as isize) as *mut u64x2) = v0[139]};
                    unsafe{*(area.offset(280 as isize) as *mut u64x2) = v0[140]};
                    unsafe{*(area.offset(282 as isize) as *mut u64x2) = v0[141]};
                    unsafe{*(area.offset(284 as isize) as *mut u64x2) = v0[142]};
                    unsafe{*(area.offset(286 as isize) as *mut u64x2) = v0[143]};
                    unsafe{*(area.offset(288 as isize) as *mut u64x2) = v0[144]};
                    unsafe{*(area.offset(290 as isize) as *mut u64x2) = v0[145]};
                    unsafe{*(area.offset(292 as isize) as *mut u64x2) = v0[146]};
                    unsafe{*(area.offset(294 as isize) as *mut u64x2) = v0[147]};
                    unsafe{*(area.offset(296 as isize) as *mut u64x2) = v0[148]};
                    unsafe{*(area.offset(298 as isize) as *mut u64x2) = v0[149]};
                    unsafe{*(area.offset(300 as isize) as *mut u64x2) = v0[150]};
                    unsafe{*(area.offset(302 as isize) as *mut u64x2) = v0[151]};
                    unsafe{*(area.offset(304 as isize) as *mut u64x2) = v0[152]};
                    unsafe{*(area.offset(306 as isize) as *mut u64x2) = v0[153]};
                    unsafe{*(area.offset(308 as isize) as *mut u64x2) = v0[154]};
                    unsafe{*(area.offset(310 as isize) as *mut u64x2) = v0[155]};
                    unsafe{*(area.offset(312 as isize) as *mut u64x2) = v0[156]};
                    unsafe{*(area.offset(314 as isize) as *mut u64x2) = v0[157]};
                    unsafe{*(area.offset(316 as isize) as *mut u64x2) = v0[158]};
                    unsafe{*(area.offset(318 as isize) as *mut u64x2) = v0[159]};
                    unsafe{*(area.offset(320 as isize) as *mut u64x2) = v0[160]};
                    unsafe{*(area.offset(322 as isize) as *mut u64x2) = v0[161]};
                    unsafe{*(area.offset(324 as isize) as *mut u64x2) = v0[162]};
                    unsafe{*(area.offset(326 as isize) as *mut u64x2) = v0[163]};
                    unsafe{*(area.offset(328 as isize) as *mut u64x2) = v0[164]};
                    unsafe{*(area.offset(330 as isize) as *mut u64x2) = v0[165]};
                    unsafe{*(area.offset(332 as isize) as *mut u64x2) = v0[166]};
                    unsafe{*(area.offset(334 as isize) as *mut u64x2) = v0[167]};
                    unsafe{*(area.offset(336 as isize) as *mut u64x2) = v0[168]};
                    unsafe{*(area.offset(338 as isize) as *mut u64x2) = v0[169]};
                    unsafe{*(area.offset(340 as isize) as *mut u64x2) = v0[170]};
                    unsafe{*(area.offset(342 as isize) as *mut u64x2) = v0[171]};
                    unsafe{*(area.offset(344 as isize) as *mut u64x2) = v0[172]};
                    unsafe{*(area.offset(346 as isize) as *mut u64x2) = v0[173]};
                    unsafe{*(area.offset(348 as isize) as *mut u64x2) = v0[174]};
                    unsafe{*(area.offset(350 as isize) as *mut u64x2) = v0[175]};
                    unsafe{*(area.offset(352 as isize) as *mut u64x2) = v0[176]};
                    unsafe{*(area.offset(354 as isize) as *mut u64x2) = v0[177]};
                    unsafe{*(area.offset(356 as isize) as *mut u64x2) = v0[178]};
                    unsafe{*(area.offset(358 as isize) as *mut u64x2) = v0[179]};
                    unsafe{*(area.offset(360 as isize) as *mut u64x2) = v0[180]};
                    unsafe{*(area.offset(362 as isize) as *mut u64x2) = v0[181]};
                    unsafe{*(area.offset(364 as isize) as *mut u64x2) = v0[182]};
                    unsafe{*(area.offset(366 as isize) as *mut u64x2) = v0[183]};
                    unsafe{*(area.offset(368 as isize) as *mut u64x2) = v0[184]};
                    unsafe{*(area.offset(370 as isize) as *mut u64x2) = v0[185]};
                    unsafe{*(area.offset(372 as isize) as *mut u64x2) = v0[186]};
                    unsafe{*(area.offset(374 as isize) as *mut u64x2) = v0[187]};
                    unsafe{*(area.offset(376 as isize) as *mut u64x2) = v0[188]};
                    unsafe{*(area.offset(378 as isize) as *mut u64x2) = v0[189]};
                    unsafe{*(area.offset(380 as isize) as *mut u64x2) = v0[190]};
                    unsafe{*(area.offset(382 as isize) as *mut u64x2) = v0[191]};
                    unsafe{*(area.offset(384 as isize) as *mut u64x2) = v0[192]};
                    unsafe{*(area.offset(386 as isize) as *mut u64x2) = v0[193]};
                    unsafe{*(area.offset(388 as isize) as *mut u64x2) = v0[194]};
                    unsafe{*(area.offset(390 as isize) as *mut u64x2) = v0[195]};
                    unsafe{*(area.offset(392 as isize) as *mut u64x2) = v0[196]};
                    unsafe{*(area.offset(394 as isize) as *mut u64x2) = v0[197]};
                    unsafe{*(area.offset(396 as isize) as *mut u64x2) = v0[198]};
                    unsafe{*(area.offset(398 as isize) as *mut u64x2) = v0[199]};
                    unsafe{*(area.offset(400 as isize) as *mut u64x2) = v0[200]};
                    unsafe{*(area.offset(402 as isize) as *mut u64x2) = v0[201]};
                    unsafe{*(area.offset(404 as isize) as *mut u64x2) = v0[202]};
                    unsafe{*(area.offset(406 as isize) as *mut u64x2) = v0[203]};
                    unsafe{*(area.offset(408 as isize) as *mut u64x2) = v0[204]};
                    unsafe{*(area.offset(410 as isize) as *mut u64x2) = v0[205]};
                    unsafe{*(area.offset(412 as isize) as *mut u64x2) = v0[206]};
                    unsafe{*(area.offset(414 as isize) as *mut u64x2) = v0[207]};
                    unsafe{*(area.offset(416 as isize) as *mut u64x2) = v0[208]};
                    unsafe{*(area.offset(418 as isize) as *mut u64x2) = v0[209]};
                    unsafe{*(area.offset(420 as isize) as *mut u64x2) = v0[210]};
                    unsafe{*(area.offset(422 as isize) as *mut u64x2) = v0[211]};
                    unsafe{*(area.offset(424 as isize) as *mut u64x2) = v0[212]};
                    unsafe{*(area.offset(426 as isize) as *mut u64x2) = v0[213]};
                    unsafe{*(area.offset(428 as isize) as *mut u64x2) = v0[214]};
                    unsafe{*(area.offset(430 as isize) as *mut u64x2) = v0[215]};
                    unsafe{*(area.offset(432 as isize) as *mut u64x2) = v0[216]};
                    unsafe{*(area.offset(434 as isize) as *mut u64x2) = v0[217]};
                    unsafe{*(area.offset(436 as isize) as *mut u64x2) = v0[218]};
                    unsafe{*(area.offset(438 as isize) as *mut u64x2) = v0[219]};
                    unsafe{*(area.offset(440 as isize) as *mut u64x2) = v0[220]};
                    unsafe{*(area.offset(442 as isize) as *mut u64x2) = v0[221]};
                    unsafe{*(area.offset(444 as isize) as *mut u64x2) = v0[222]};
                    unsafe{*(area.offset(446 as isize) as *mut u64x2) = v0[223]};
                    unsafe{*(area.offset(448 as isize) as *mut u64x2) = v0[224]};
                    unsafe{*(area.offset(450 as isize) as *mut u64x2) = v0[225]};
                    unsafe{*(area.offset(452 as isize) as *mut u64x2) = v0[226]};
                    unsafe{*(area.offset(454 as isize) as *mut u64x2) = v0[227]};
                    unsafe{*(area.offset(456 as isize) as *mut u64x2) = v0[228]};
                    unsafe{*(area.offset(458 as isize) as *mut u64x2) = v0[229]};
                    unsafe{*(area.offset(460 as isize) as *mut u64x2) = v0[230]};
                    unsafe{*(area.offset(462 as isize) as *mut u64x2) = v0[231]};
                    unsafe{*(area.offset(464 as isize) as *mut u64x2) = v0[232]};
                    unsafe{*(area.offset(466 as isize) as *mut u64x2) = v0[233]};
                    unsafe{*(area.offset(468 as isize) as *mut u64x2) = v0[234]};
                    unsafe{*(area.offset(470 as isize) as *mut u64x2) = v0[235]};
                    unsafe{*(area.offset(472 as isize) as *mut u64x2) = v0[236]};
                    unsafe{*(area.offset(474 as isize) as *mut u64x2) = v0[237]};
                    unsafe{*(area.offset(476 as isize) as *mut u64x2) = v0[238]};
                    unsafe{*(area.offset(478 as isize) as *mut u64x2) = v0[239]};
                    unsafe{*(area.offset(480 as isize) as *mut u64x2) = v0[240]};
                    unsafe{*(area.offset(482 as isize) as *mut u64x2) = v0[241]};
                    unsafe{*(area.offset(484 as isize) as *mut u64x2) = v0[242]};
                    unsafe{*(area.offset(486 as isize) as *mut u64x2) = v0[243]};
                    unsafe{*(area.offset(488 as isize) as *mut u64x2) = v0[244]};
                    unsafe{*(area.offset(490 as isize) as *mut u64x2) = v0[245]};
                    unsafe{*(area.offset(492 as isize) as *mut u64x2) = v0[246]};
                    unsafe{*(area.offset(494 as isize) as *mut u64x2) = v0[247]};
                    unsafe{*(area.offset(496 as isize) as *mut u64x2) = v0[248]};
                    unsafe{*(area.offset(498 as isize) as *mut u64x2) = v0[249]};
                    unsafe{*(area.offset(500 as isize) as *mut u64x2) = v0[250]};
                    unsafe{*(area.offset(502 as isize) as *mut u64x2) = v0[251]};
                    unsafe{*(area.offset(504 as isize) as *mut u64x2) = v0[252]};
                    unsafe{*(area.offset(506 as isize) as *mut u64x2) = v0[253]};
                    unsafe{*(area.offset(508 as isize) as *mut u64x2) = v0[254]};
                    unsafe{*(area.offset(510 as isize) as *mut u64x2) = v0[255]};
                    unsafe{*(area.offset(512 as isize) as *mut u64x2) = v0[256]};
                    unsafe{*(area.offset(514 as isize) as *mut u64x2) = v0[257]};
                    unsafe{*(area.offset(516 as isize) as *mut u64x2) = v0[258]};
                    unsafe{*(area.offset(518 as isize) as *mut u64x2) = v0[259]};
                    unsafe{*(area.offset(520 as isize) as *mut u64x2) = v0[260]};
                    unsafe{*(area.offset(522 as isize) as *mut u64x2) = v0[261]};
                    unsafe{*(area.offset(524 as isize) as *mut u64x2) = v0[262]};
                    unsafe{*(area.offset(526 as isize) as *mut u64x2) = v0[263]};
                    unsafe{*(area.offset(528 as isize) as *mut u64x2) = v0[264]};
                    unsafe{*(area.offset(530 as isize) as *mut u64x2) = v0[265]};
                    unsafe{*(area.offset(532 as isize) as *mut u64x2) = v0[266]};
                    unsafe{*(area.offset(534 as isize) as *mut u64x2) = v0[267]};
                    unsafe{*(area.offset(536 as isize) as *mut u64x2) = v0[268]};
                    unsafe{*(area.offset(538 as isize) as *mut u64x2) = v0[269]};
                    unsafe{*(area.offset(540 as isize) as *mut u64x2) = v0[270]};
                    unsafe{*(area.offset(542 as isize) as *mut u64x2) = v0[271]};
                    unsafe{*(area.offset(544 as isize) as *mut u64x2) = v0[272]};
                    unsafe{*(area.offset(546 as isize) as *mut u64x2) = v0[273]};
                    unsafe{*(area.offset(548 as isize) as *mut u64x2) = v0[274]};
                    unsafe{*(area.offset(550 as isize) as *mut u64x2) = v0[275]};
                    unsafe{*(area.offset(552 as isize) as *mut u64x2) = v0[276]};
                    unsafe{*(area.offset(554 as isize) as *mut u64x2) = v0[277]};
                    unsafe{*(area.offset(556 as isize) as *mut u64x2) = v0[278]};
                    unsafe{*(area.offset(558 as isize) as *mut u64x2) = v0[279]};
                    unsafe{*(area.offset(560 as isize) as *mut u64x2) = v0[280]};
                    unsafe{*(area.offset(562 as isize) as *mut u64x2) = v0[281]};
                    unsafe{*(area.offset(564 as isize) as *mut u64x2) = v0[282]};
                    unsafe{*(area.offset(566 as isize) as *mut u64x2) = v0[283]};
                    unsafe{*(area.offset(568 as isize) as *mut u64x2) = v0[284]};
                    unsafe{*(area.offset(570 as isize) as *mut u64x2) = v0[285]};
                    unsafe{*(area.offset(572 as isize) as *mut u64x2) = v0[286]};
                    unsafe{*(area.offset(574 as isize) as *mut u64x2) = v0[287]};
                    unsafe{*(area.offset(576 as isize) as *mut u64x2) = v0[288]};
                    unsafe{*(area.offset(578 as isize) as *mut u64x2) = v0[289]};
                    unsafe{*(area.offset(580 as isize) as *mut u64x2) = v0[290]};
                    unsafe{*(area.offset(582 as isize) as *mut u64x2) = v0[291]};
                    unsafe{*(area.offset(584 as isize) as *mut u64x2) = v0[292]};
                    unsafe{*(area.offset(586 as isize) as *mut u64x2) = v0[293]};
                    unsafe{*(area.offset(588 as isize) as *mut u64x2) = v0[294]};
                    unsafe{*(area.offset(590 as isize) as *mut u64x2) = v0[295]};
                    unsafe{*(area.offset(592 as isize) as *mut u64x2) = v0[296]};
                    unsafe{*(area.offset(594 as isize) as *mut u64x2) = v0[297]};
                    unsafe{*(area.offset(596 as isize) as *mut u64x2) = v0[298]};
                    unsafe{*(area.offset(598 as isize) as *mut u64x2) = v0[299]};
                    unsafe{*(area.offset(600 as isize) as *mut u64x2) = v0[300]};
                    unsafe{*(area.offset(602 as isize) as *mut u64x2) = v0[301]};
                    unsafe{*(area.offset(604 as isize) as *mut u64x2) = v0[302]};
                    unsafe{*(area.offset(606 as isize) as *mut u64x2) = v0[303]};
                    unsafe{*(area.offset(608 as isize) as *mut u64x2) = v0[304]};
                    unsafe{*(area.offset(610 as isize) as *mut u64x2) = v0[305]};
                    unsafe{*(area.offset(612 as isize) as *mut u64x2) = v0[306]};
                    unsafe{*(area.offset(614 as isize) as *mut u64x2) = v0[307]};
                    unsafe{*(area.offset(616 as isize) as *mut u64x2) = v0[308]};
                    unsafe{*(area.offset(618 as isize) as *mut u64x2) = v0[309]};
                    unsafe{*(area.offset(620 as isize) as *mut u64x2) = v0[310]};
                    unsafe{*(area.offset(622 as isize) as *mut u64x2) = v0[311]};
                    unsafe{*(area.offset(624 as isize) as *mut u64x2) = v0[312]};
                    unsafe{*(area.offset(626 as isize) as *mut u64x2) = v0[313]};
                    unsafe{*(area.offset(628 as isize) as *mut u64x2) = v0[314]};
                    unsafe{*(area.offset(630 as isize) as *mut u64x2) = v0[315]};
                    unsafe{*(area.offset(632 as isize) as *mut u64x2) = v0[316]};
                    unsafe{*(area.offset(634 as isize) as *mut u64x2) = v0[317]};
                    unsafe{*(area.offset(636 as isize) as *mut u64x2) = v0[318]};
                    unsafe{*(area.offset(638 as isize) as *mut u64x2) = v0[319]};
                    unsafe{*(area.offset(640 as isize) as *mut u64x2) = v0[320]};
                    unsafe{*(area.offset(642 as isize) as *mut u64x2) = v0[321]};
                    unsafe{*(area.offset(644 as isize) as *mut u64x2) = v0[322]};
                    unsafe{*(area.offset(646 as isize) as *mut u64x2) = v0[323]};
                    unsafe{*(area.offset(648 as isize) as *mut u64x2) = v0[324]};
                    unsafe{*(area.offset(650 as isize) as *mut u64x2) = v0[325]};
                    unsafe{*(area.offset(652 as isize) as *mut u64x2) = v0[326]};
                    unsafe{*(area.offset(654 as isize) as *mut u64x2) = v0[327]};
                    unsafe{*(area.offset(656 as isize) as *mut u64x2) = v0[328]};
                    unsafe{*(area.offset(658 as isize) as *mut u64x2) = v0[329]};
                    unsafe{*(area.offset(660 as isize) as *mut u64x2) = v0[330]};
                    unsafe{*(area.offset(662 as isize) as *mut u64x2) = v0[331]};
                    unsafe{*(area.offset(664 as isize) as *mut u64x2) = v0[332]};
                    unsafe{*(area.offset(666 as isize) as *mut u64x2) = v0[333]};
                    unsafe{*(area.offset(668 as isize) as *mut u64x2) = v0[334]};
                    unsafe{*(area.offset(670 as isize) as *mut u64x2) = v0[335]};
                    unsafe{*(area.offset(672 as isize) as *mut u64x2) = v0[336]};
                    unsafe{*(area.offset(674 as isize) as *mut u64x2) = v0[337]};
                    unsafe{*(area.offset(676 as isize) as *mut u64x2) = v0[338]};
                    unsafe{*(area.offset(678 as isize) as *mut u64x2) = v0[339]};
                    unsafe{*(area.offset(680 as isize) as *mut u64x2) = v0[340]};
                    unsafe{*(area.offset(682 as isize) as *mut u64x2) = v0[341]};
                    unsafe{*(area.offset(684 as isize) as *mut u64x2) = v0[342]};
                    unsafe{*(area.offset(686 as isize) as *mut u64x2) = v0[343]};
                    unsafe{*(area.offset(688 as isize) as *mut u64x2) = v0[344]};
                    unsafe{*(area.offset(690 as isize) as *mut u64x2) = v0[345]};
                    unsafe{*(area.offset(692 as isize) as *mut u64x2) = v0[346]};
                    unsafe{*(area.offset(694 as isize) as *mut u64x2) = v0[347]};
                    unsafe{*(area.offset(696 as isize) as *mut u64x2) = v0[348]};
                    unsafe{*(area.offset(698 as isize) as *mut u64x2) = v0[349]};
                    unsafe{*(area.offset(700 as isize) as *mut u64x2) = v0[350]};
                    unsafe{*(area.offset(702 as isize) as *mut u64x2) = v0[351]};
                    unsafe{*(area.offset(704 as isize) as *mut u64x2) = v0[352]};
                    unsafe{*(area.offset(706 as isize) as *mut u64x2) = v0[353]};
                    unsafe{*(area.offset(708 as isize) as *mut u64x2) = v0[354]};
                    unsafe{*(area.offset(710 as isize) as *mut u64x2) = v0[355]};
                    unsafe{*(area.offset(712 as isize) as *mut u64x2) = v0[356]};
                    unsafe{*(area.offset(714 as isize) as *mut u64x2) = v0[357]};
                    unsafe{*(area.offset(716 as isize) as *mut u64x2) = v0[358]};
                    unsafe{*(area.offset(718 as isize) as *mut u64x2) = v0[359]};
                    unsafe{*(area.offset(720 as isize) as *mut u64x2) = v0[360]};
                    unsafe{*(area.offset(722 as isize) as *mut u64x2) = v0[361]};
                    unsafe{*(area.offset(724 as isize) as *mut u64x2) = v0[362]};
                    unsafe{*(area.offset(726 as isize) as *mut u64x2) = v0[363]};
                    unsafe{*(area.offset(728 as isize) as *mut u64x2) = v0[364]};
                    unsafe{*(area.offset(730 as isize) as *mut u64x2) = v0[365]};
                    unsafe{*(area.offset(732 as isize) as *mut u64x2) = v0[366]};
                    unsafe{*(area.offset(734 as isize) as *mut u64x2) = v0[367]};
                    unsafe{*(area.offset(736 as isize) as *mut u64x2) = v0[368]};
                    unsafe{*(area.offset(738 as isize) as *mut u64x2) = v0[369]};
                    unsafe{*(area.offset(740 as isize) as *mut u64x2) = v0[370]};
                    unsafe{*(area.offset(742 as isize) as *mut u64x2) = v0[371]};
                    unsafe{*(area.offset(744 as isize) as *mut u64x2) = v0[372]};
                    unsafe{*(area.offset(746 as isize) as *mut u64x2) = v0[373]};
                    unsafe{*(area.offset(748 as isize) as *mut u64x2) = v0[374]};
                    unsafe{*(area.offset(750 as isize) as *mut u64x2) = v0[375]};
                    unsafe{*(area.offset(752 as isize) as *mut u64x2) = v0[376]};
                    unsafe{*(area.offset(754 as isize) as *mut u64x2) = v0[377]};
                    unsafe{*(area.offset(756 as isize) as *mut u64x2) = v0[378]};
                    unsafe{*(area.offset(758 as isize) as *mut u64x2) = v0[379]};
                    unsafe{*(area.offset(760 as isize) as *mut u64x2) = v0[380]};
                    unsafe{*(area.offset(762 as isize) as *mut u64x2) = v0[381]};
                    unsafe{*(area.offset(764 as isize) as *mut u64x2) = v0[382]};
                    unsafe{*(area.offset(766 as isize) as *mut u64x2) = v0[383]};
                    unsafe{*(area.offset(768 as isize) as *mut u64x2) = v0[384]};
                    unsafe{*(area.offset(770 as isize) as *mut u64x2) = v0[385]};
                    unsafe{*(area.offset(772 as isize) as *mut u64x2) = v0[386]};
                    unsafe{*(area.offset(774 as isize) as *mut u64x2) = v0[387]};
                    unsafe{*(area.offset(776 as isize) as *mut u64x2) = v0[388]};
                    unsafe{*(area.offset(778 as isize) as *mut u64x2) = v0[389]};
                    unsafe{*(area.offset(780 as isize) as *mut u64x2) = v0[390]};
                    unsafe{*(area.offset(782 as isize) as *mut u64x2) = v0[391]};
                    unsafe{*(area.offset(784 as isize) as *mut u64x2) = v0[392]};
                    unsafe{*(area.offset(786 as isize) as *mut u64x2) = v0[393]};
                    unsafe{*(area.offset(788 as isize) as *mut u64x2) = v0[394]};
                    unsafe{*(area.offset(790 as isize) as *mut u64x2) = v0[395]};
                    unsafe{*(area.offset(792 as isize) as *mut u64x2) = v0[396]};
                    unsafe{*(area.offset(794 as isize) as *mut u64x2) = v0[397]};
                    unsafe{*(area.offset(796 as isize) as *mut u64x2) = v0[398]};
                    unsafe{*(area.offset(798 as isize) as *mut u64x2) = v0[399]};
                    unsafe{*(area.offset(800 as isize) as *mut u64x2) = v0[400]};
                    unsafe{*(area.offset(802 as isize) as *mut u64x2) = v0[401]};
                    unsafe{*(area.offset(804 as isize) as *mut u64x2) = v0[402]};
                    unsafe{*(area.offset(806 as isize) as *mut u64x2) = v0[403]};
                    unsafe{*(area.offset(808 as isize) as *mut u64x2) = v0[404]};
                    unsafe{*(area.offset(810 as isize) as *mut u64x2) = v0[405]};
                    unsafe{*(area.offset(812 as isize) as *mut u64x2) = v0[406]};
                    unsafe{*(area.offset(814 as isize) as *mut u64x2) = v0[407]};
                    unsafe{*(area.offset(816 as isize) as *mut u64x2) = v0[408]};
                    unsafe{*(area.offset(818 as isize) as *mut u64x2) = v0[409]};
                    unsafe{*(area.offset(820 as isize) as *mut u64x2) = v0[410]};
                    unsafe{*(area.offset(822 as isize) as *mut u64x2) = v0[411]};
                    unsafe{*(area.offset(824 as isize) as *mut u64x2) = v0[412]};
                    unsafe{*(area.offset(826 as isize) as *mut u64x2) = v0[413]};
                    unsafe{*(area.offset(828 as isize) as *mut u64x2) = v0[414]};
                    unsafe{*(area.offset(830 as isize) as *mut u64x2) = v0[415]};
                    unsafe{*(area.offset(832 as isize) as *mut u64x2) = v0[416]};
                    unsafe{*(area.offset(834 as isize) as *mut u64x2) = v0[417]};
                    unsafe{*(area.offset(836 as isize) as *mut u64x2) = v0[418]};
                    unsafe{*(area.offset(838 as isize) as *mut u64x2) = v0[419]};
                    unsafe{*(area.offset(840 as isize) as *mut u64x2) = v0[420]};
                    unsafe{*(area.offset(842 as isize) as *mut u64x2) = v0[421]};
                    unsafe{*(area.offset(844 as isize) as *mut u64x2) = v0[422]};
                    unsafe{*(area.offset(846 as isize) as *mut u64x2) = v0[423]};
                    unsafe{*(area.offset(848 as isize) as *mut u64x2) = v0[424]};
                    unsafe{*(area.offset(850 as isize) as *mut u64x2) = v0[425]};
                    unsafe{*(area.offset(852 as isize) as *mut u64x2) = v0[426]};
                    unsafe{*(area.offset(854 as isize) as *mut u64x2) = v0[427]};
                    unsafe{*(area.offset(856 as isize) as *mut u64x2) = v0[428]};
                    unsafe{*(area.offset(858 as isize) as *mut u64x2) = v0[429]};
                    unsafe{*(area.offset(860 as isize) as *mut u64x2) = v0[430]};
                    unsafe{*(area.offset(862 as isize) as *mut u64x2) = v0[431]};
                    unsafe{*(area.offset(864 as isize) as *mut u64x2) = v0[432]};
                    unsafe{*(area.offset(866 as isize) as *mut u64x2) = v0[433]};
                    unsafe{*(area.offset(868 as isize) as *mut u64x2) = v0[434]};
                    unsafe{*(area.offset(870 as isize) as *mut u64x2) = v0[435]};
                    unsafe{*(area.offset(872 as isize) as *mut u64x2) = v0[436]};
                    unsafe{*(area.offset(874 as isize) as *mut u64x2) = v0[437]};
                    unsafe{*(area.offset(876 as isize) as *mut u64x2) = v0[438]};
                    unsafe{*(area.offset(878 as isize) as *mut u64x2) = v0[439]};
                    unsafe{*(area.offset(880 as isize) as *mut u64x2) = v0[440]};
                    unsafe{*(area.offset(882 as isize) as *mut u64x2) = v0[441]};
                    unsafe{*(area.offset(884 as isize) as *mut u64x2) = v0[442]};
                    unsafe{*(area.offset(886 as isize) as *mut u64x2) = v0[443]};
                    unsafe{*(area.offset(888 as isize) as *mut u64x2) = v0[444]};
                    unsafe{*(area.offset(890 as isize) as *mut u64x2) = v0[445]};
                    unsafe{*(area.offset(892 as isize) as *mut u64x2) = v0[446]};
                    unsafe{*(area.offset(894 as isize) as *mut u64x2) = v0[447]};
                    unsafe{*(area.offset(896 as isize) as *mut u64x2) = v0[448]};
                    unsafe{*(area.offset(898 as isize) as *mut u64x2) = v0[449]};
                    unsafe{*(area.offset(900 as isize) as *mut u64x2) = v0[450]};
                    unsafe{*(area.offset(902 as isize) as *mut u64x2) = v0[451]};
                    unsafe{*(area.offset(904 as isize) as *mut u64x2) = v0[452]};
                    unsafe{*(area.offset(906 as isize) as *mut u64x2) = v0[453]};
                    unsafe{*(area.offset(908 as isize) as *mut u64x2) = v0[454]};
                    unsafe{*(area.offset(910 as isize) as *mut u64x2) = v0[455]};
                    unsafe{*(area.offset(912 as isize) as *mut u64x2) = v0[456]};
                    unsafe{*(area.offset(914 as isize) as *mut u64x2) = v0[457]};
                    unsafe{*(area.offset(916 as isize) as *mut u64x2) = v0[458]};
                    unsafe{*(area.offset(918 as isize) as *mut u64x2) = v0[459]};
                    unsafe{*(area.offset(920 as isize) as *mut u64x2) = v0[460]};
                    unsafe{*(area.offset(922 as isize) as *mut u64x2) = v0[461]};
                    unsafe{*(area.offset(924 as isize) as *mut u64x2) = v0[462]};
                    unsafe{*(area.offset(926 as isize) as *mut u64x2) = v0[463]};
                    unsafe{*(area.offset(928 as isize) as *mut u64x2) = v0[464]};
                    unsafe{*(area.offset(930 as isize) as *mut u64x2) = v0[465]};
                    unsafe{*(area.offset(932 as isize) as *mut u64x2) = v0[466]};
                    unsafe{*(area.offset(934 as isize) as *mut u64x2) = v0[467]};
                    unsafe{*(area.offset(936 as isize) as *mut u64x2) = v0[468]};
                    unsafe{*(area.offset(938 as isize) as *mut u64x2) = v0[469]};
                    unsafe{*(area.offset(940 as isize) as *mut u64x2) = v0[470]};
                    unsafe{*(area.offset(942 as isize) as *mut u64x2) = v0[471]};
                    unsafe{*(area.offset(944 as isize) as *mut u64x2) = v0[472]};
                    unsafe{*(area.offset(946 as isize) as *mut u64x2) = v0[473]};
                    unsafe{*(area.offset(948 as isize) as *mut u64x2) = v0[474]};
                    unsafe{*(area.offset(950 as isize) as *mut u64x2) = v0[475]};
                    unsafe{*(area.offset(952 as isize) as *mut u64x2) = v0[476]};
                    unsafe{*(area.offset(954 as isize) as *mut u64x2) = v0[477]};
                    unsafe{*(area.offset(956 as isize) as *mut u64x2) = v0[478]};
                    unsafe{*(area.offset(958 as isize) as *mut u64x2) = v0[479]};
                    unsafe{*(area.offset(960 as isize) as *mut u64x2) = v0[480]};
                    unsafe{*(area.offset(962 as isize) as *mut u64x2) = v0[481]};
                    unsafe{*(area.offset(964 as isize) as *mut u64x2) = v0[482]};
                    unsafe{*(area.offset(966 as isize) as *mut u64x2) = v0[483]};
                    unsafe{*(area.offset(968 as isize) as *mut u64x2) = v0[484]};
                    unsafe{*(area.offset(970 as isize) as *mut u64x2) = v0[485]};
                    unsafe{*(area.offset(972 as isize) as *mut u64x2) = v0[486]};
                    unsafe{*(area.offset(974 as isize) as *mut u64x2) = v0[487]};
                    unsafe{*(area.offset(976 as isize) as *mut u64x2) = v0[488]};
                    unsafe{*(area.offset(978 as isize) as *mut u64x2) = v0[489]};
                    unsafe{*(area.offset(980 as isize) as *mut u64x2) = v0[490]};
                    unsafe{*(area.offset(982 as isize) as *mut u64x2) = v0[491]};
                    unsafe{*(area.offset(984 as isize) as *mut u64x2) = v0[492]};
                    unsafe{*(area.offset(986 as isize) as *mut u64x2) = v0[493]};
                    unsafe{*(area.offset(988 as isize) as *mut u64x2) = v0[494]};
                    unsafe{*(area.offset(990 as isize) as *mut u64x2) = v0[495]};
                    unsafe{*(area.offset(992 as isize) as *mut u64x2) = v0[496]};
                    unsafe{*(area.offset(994 as isize) as *mut u64x2) = v0[497]};
                    unsafe{*(area.offset(996 as isize) as *mut u64x2) = v0[498]};
                    unsafe{*(area.offset(998 as isize) as *mut u64x2) = v0[499]};
                    unsafe{*(area.offset(1000 as isize) as *mut u64x2) = v0[500]};
                    unsafe{*(area.offset(1002 as isize) as *mut u64x2) = v0[501]};
                    unsafe{*(area.offset(1004 as isize) as *mut u64x2) = v0[502]};
                    unsafe{*(area.offset(1006 as isize) as *mut u64x2) = v0[503]};
                    unsafe{*(area.offset(1008 as isize) as *mut u64x2) = v0[504]};
                    unsafe{*(area.offset(1010 as isize) as *mut u64x2) = v0[505]};
                    unsafe{*(area.offset(1012 as isize) as *mut u64x2) = v0[506]};
                    unsafe{*(area.offset(1014 as isize) as *mut u64x2) = v0[507]};
                    unsafe{*(area.offset(1016 as isize) as *mut u64x2) = v0[508]};
                    unsafe{*(area.offset(1018 as isize) as *mut u64x2) = v0[509]};
                    unsafe{*(area.offset(1020 as isize) as *mut u64x2) = v0[510]};
                    unsafe{*(area.offset(1022 as isize) as *mut u64x2) = v0[511]};
                    unsafe{*(area.offset(1024 as isize) as *mut u64x2) = v0[512]};
                    unsafe{*(area.offset(1026 as isize) as *mut u64x2) = v0[513]};
                    unsafe{*(area.offset(1028 as isize) as *mut u64x2) = v0[514]};
                    unsafe{*(area.offset(1030 as isize) as *mut u64x2) = v0[515]};
                    unsafe{*(area.offset(1032 as isize) as *mut u64x2) = v0[516]};
                    unsafe{*(area.offset(1034 as isize) as *mut u64x2) = v0[517]};
                    unsafe{*(area.offset(1036 as isize) as *mut u64x2) = v0[518]};
                    unsafe{*(area.offset(1038 as isize) as *mut u64x2) = v0[519]};
                    unsafe{*(area.offset(1040 as isize) as *mut u64x2) = v0[520]};
                    unsafe{*(area.offset(1042 as isize) as *mut u64x2) = v0[521]};
                    unsafe{*(area.offset(1044 as isize) as *mut u64x2) = v0[522]};
                    unsafe{*(area.offset(1046 as isize) as *mut u64x2) = v0[523]};
                    unsafe{*(area.offset(1048 as isize) as *mut u64x2) = v0[524]};
                    unsafe{*(area.offset(1050 as isize) as *mut u64x2) = v0[525]};
                    unsafe{*(area.offset(1052 as isize) as *mut u64x2) = v0[526]};
                    unsafe{*(area.offset(1054 as isize) as *mut u64x2) = v0[527]};

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
                    unsafe { pc = *(area.offset(1024 + 0 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(1024 + 2 as isize) as *mut i64x2); }
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
                    unsafe { pc = *(area.offset(1024 + 4 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(1024 + 6 as isize) as *mut i64x2); }
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
                    unsafe { pc = *(area.offset(1024 + 8 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(1024 + 10 as isize) as *mut i64x2); }
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
                    unsafe { pc = *(area.offset(1024 + 12 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(1024 + 14 as isize) as *mut i64x2); }
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
                    unsafe{pc = *(area.offset(1024 + 16 as isize) as *mut i64x2);}
                    dmb();
                    unsafe{it = *(area.offset(1024 + 18 as isize) as *mut i64x2);}
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
                    unsafe { pc = *(area.offset(1024 + 20 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(1024 + 22 as isize) as *mut i64x2); }
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
                    unsafe { pc = *(area.offset(1024 + 24 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(1024 + 26 as isize) as *mut i64x2); }
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
                    unsafe { pc = *(area.offset(1024 + 28 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(1024 + 30 as isize) as *mut i64x2); }
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
                    unsafe { pc = *(area.offset(1024 + 32 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(1024 + 34 as isize) as *mut i64x2); }
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
                    unsafe { pc = *(area.offset(1024 + 36 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(1024 + 38 as isize) as *mut i64x2); }
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
                    unsafe { pc = *(area.offset(1024 + 40 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(1024 + 42 as isize) as *mut i64x2); }
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
                    unsafe { pc = *(area.offset(1024 + 44 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(1024 + 46 as isize) as *mut i64x2); }
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
                    unsafe { pc = *(area.offset(1024 + 48 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(1024 + 50 as isize) as *mut i64x2); }
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
                    unsafe { pc = *(area.offset(1024 + 52 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(1024 + 54 as isize) as *mut i64x2); }
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
                    unsafe { pc = *(area.offset(1024 + 56 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(1024 + 58 as isize) as *mut i64x2); }
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
                    unsafe { pc = *(area.offset(1024 + 60 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(1024 + 62 as isize) as *mut i64x2); }
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
                    unsafe { pc = *(area.offset(1024 + 64 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(1024 + 66 as isize) as *mut i64x2); }
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
                    unsafe { pc = *(area.offset(1024 + 68 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(1024 + 70 as isize) as *mut i64x2); }
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
                    unsafe { pc = *(area.offset(1024 + 72 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(1024 + 74 as isize) as *mut i64x2); }
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
                    unsafe { pc = *(area.offset(1024 + 76 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(1024 + 78 as isize) as *mut i64x2); }
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
                    unsafe { pc = *(area.offset(1024 + 80 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(1024 + 82 as isize) as *mut i64x2); }
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
                    unsafe { pc = *(area.offset(1024 + 84 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(1024 + 86 as isize) as *mut i64x2); }
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
                    unsafe { pc = *(area.offset(1024 + 88 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(1024 + 90 as isize) as *mut i64x2); }
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
                    unsafe { pc = *(area.offset(1024 + 92 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(1024 + 94 as isize) as *mut i64x2); }
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
                    unsafe { pc = *(area.offset(1024 + 96 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(1024 + 98 as isize) as *mut i64x2); }
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
                    unsafe { pc = *(area.offset(1024 + 100 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(1024 + 102 as isize) as *mut i64x2); }
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
                    unsafe { pc = *(area.offset(1024 + 104 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(1024 + 106 as isize) as *mut i64x2); }
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
                    unsafe { pc = *(area.offset(1024 + 108 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(1024 + 110 as isize) as *mut i64x2); }
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
                    unsafe { pc = *(area.offset(1024 + 112 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(1024 + 114 as isize) as *mut i64x2); }
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
                    unsafe { pc = *(area.offset(1024 + 116 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(1024 + 118 as isize) as *mut i64x2); }
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
                    unsafe { pc = *(area.offset(1024 + 120 as isize) as *mut i64x2); }
                    dmb();
                    unsafe { it = *(area.offset(1024 + 122 as isize) as *mut i64x2); }
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
                    if frontier1.len() == 0 {
                        let x =  u64x2::from_array([0, 0]);
                        v0.push(x);
                        current_length += 2;
                    } else {

                        // for now we will assume that frontier has length 1, if it is not 1 , then we already might want to modify logic on the
                        // FPGA side as well
                        let x =  u64x2::from_array([(frontier1[0] << 1) | 1u64, (frontier2[0] << 1) | 1u64]);
                        v0.push(x);
                        current_length += 2;
                    }
                }



                let frontier_last = borrow[30].borrow().frontier();
                if frontier_last.len() == 0 {
                    let x =  u64x2::from_array([0, 0]);
                    v0.push(x);
                } else {
                    let x =  u64x2::from_array([(frontier_last[0] << 1) | 1u64, 0]);
                    v0.push(x);
                }

                //println!("Current length = {}", current_length);

                for i in (0..1024).step_by(2) {
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
                    unsafe{*(area.offset(48 as isize) as *mut u64x2) = v0[24]};
                    unsafe{*(area.offset(50 as isize) as *mut u64x2) = v0[25]};
                    unsafe{*(area.offset(52 as isize) as *mut u64x2) = v0[26]};
                    unsafe{*(area.offset(54 as isize) as *mut u64x2) = v0[27]};
                    unsafe{*(area.offset(56 as isize) as *mut u64x2) = v0[28]};
                    unsafe{*(area.offset(58 as isize) as *mut u64x2) = v0[29]};
                    unsafe{*(area.offset(60 as isize) as *mut u64x2) = v0[30]};
                    unsafe{*(area.offset(62 as isize) as *mut u64x2) = v0[31]};
                    unsafe{*(area.offset(64 as isize) as *mut u64x2) = v0[32]};
                    unsafe{*(area.offset(66 as isize) as *mut u64x2) = v0[33]};
                    unsafe{*(area.offset(68 as isize) as *mut u64x2) = v0[34]};
                    unsafe{*(area.offset(70 as isize) as *mut u64x2) = v0[35]};
                    unsafe{*(area.offset(72 as isize) as *mut u64x2) = v0[36]};
                    unsafe{*(area.offset(74 as isize) as *mut u64x2) = v0[37]};
                    unsafe{*(area.offset(76 as isize) as *mut u64x2) = v0[38]};
                    unsafe{*(area.offset(78 as isize) as *mut u64x2) = v0[39]};
                    unsafe{*(area.offset(80 as isize) as *mut u64x2) = v0[40]};
                    unsafe{*(area.offset(82 as isize) as *mut u64x2) = v0[41]};
                    unsafe{*(area.offset(84 as isize) as *mut u64x2) = v0[42]};
                    unsafe{*(area.offset(86 as isize) as *mut u64x2) = v0[43]};
                    unsafe{*(area.offset(88 as isize) as *mut u64x2) = v0[44]};
                    unsafe{*(area.offset(90 as isize) as *mut u64x2) = v0[45]};
                    unsafe{*(area.offset(92 as isize) as *mut u64x2) = v0[46]};
                    unsafe{*(area.offset(94 as isize) as *mut u64x2) = v0[47]};
                    unsafe{*(area.offset(96 as isize) as *mut u64x2) = v0[48]};
                    unsafe{*(area.offset(98 as isize) as *mut u64x2) = v0[49]};
                    unsafe{*(area.offset(100 as isize) as *mut u64x2) = v0[50]};
                    unsafe{*(area.offset(102 as isize) as *mut u64x2) = v0[51]};
                    unsafe{*(area.offset(104 as isize) as *mut u64x2) = v0[52]};
                    unsafe{*(area.offset(106 as isize) as *mut u64x2) = v0[53]};
                    unsafe{*(area.offset(108 as isize) as *mut u64x2) = v0[54]};
                    unsafe{*(area.offset(110 as isize) as *mut u64x2) = v0[55]};
                    unsafe{*(area.offset(112 as isize) as *mut u64x2) = v0[56]};
                    unsafe{*(area.offset(114 as isize) as *mut u64x2) = v0[57]};
                    unsafe{*(area.offset(116 as isize) as *mut u64x2) = v0[58]};
                    unsafe{*(area.offset(118 as isize) as *mut u64x2) = v0[59]};
                    unsafe{*(area.offset(120 as isize) as *mut u64x2) = v0[60]};
                    unsafe{*(area.offset(122 as isize) as *mut u64x2) = v0[61]};
                    unsafe{*(area.offset(124 as isize) as *mut u64x2) = v0[62]};
                    unsafe{*(area.offset(126 as isize) as *mut u64x2) = v0[63]};
                    unsafe{*(area.offset(128 as isize) as *mut u64x2) = v0[64]};
                    unsafe{*(area.offset(130 as isize) as *mut u64x2) = v0[65]};
                    unsafe{*(area.offset(132 as isize) as *mut u64x2) = v0[66]};
                    unsafe{*(area.offset(134 as isize) as *mut u64x2) = v0[67]};
                    unsafe{*(area.offset(136 as isize) as *mut u64x2) = v0[68]};
                    unsafe{*(area.offset(138 as isize) as *mut u64x2) = v0[69]};
                    unsafe{*(area.offset(140 as isize) as *mut u64x2) = v0[70]};
                    unsafe{*(area.offset(142 as isize) as *mut u64x2) = v0[71]};
                    unsafe{*(area.offset(144 as isize) as *mut u64x2) = v0[72]};
                    unsafe{*(area.offset(146 as isize) as *mut u64x2) = v0[73]};
                    unsafe{*(area.offset(148 as isize) as *mut u64x2) = v0[74]};
                    unsafe{*(area.offset(150 as isize) as *mut u64x2) = v0[75]};
                    unsafe{*(area.offset(152 as isize) as *mut u64x2) = v0[76]};
                    unsafe{*(area.offset(154 as isize) as *mut u64x2) = v0[77]};
                    unsafe{*(area.offset(156 as isize) as *mut u64x2) = v0[78]};
                    unsafe{*(area.offset(158 as isize) as *mut u64x2) = v0[79]};
                    unsafe{*(area.offset(160 as isize) as *mut u64x2) = v0[80]};
                    unsafe{*(area.offset(162 as isize) as *mut u64x2) = v0[81]};
                    unsafe{*(area.offset(164 as isize) as *mut u64x2) = v0[82]};
                    unsafe{*(area.offset(166 as isize) as *mut u64x2) = v0[83]};
                    unsafe{*(area.offset(168 as isize) as *mut u64x2) = v0[84]};
                    unsafe{*(area.offset(170 as isize) as *mut u64x2) = v0[85]};
                    unsafe{*(area.offset(172 as isize) as *mut u64x2) = v0[86]};
                    unsafe{*(area.offset(174 as isize) as *mut u64x2) = v0[87]};
                    unsafe{*(area.offset(176 as isize) as *mut u64x2) = v0[88]};
                    unsafe{*(area.offset(178 as isize) as *mut u64x2) = v0[89]};
                    unsafe{*(area.offset(180 as isize) as *mut u64x2) = v0[90]};
                    unsafe{*(area.offset(182 as isize) as *mut u64x2) = v0[91]};
                    unsafe{*(area.offset(184 as isize) as *mut u64x2) = v0[92]};
                    unsafe{*(area.offset(186 as isize) as *mut u64x2) = v0[93]};
                    unsafe{*(area.offset(188 as isize) as *mut u64x2) = v0[94]};
                    unsafe{*(area.offset(190 as isize) as *mut u64x2) = v0[95]};
                    unsafe{*(area.offset(192 as isize) as *mut u64x2) = v0[96]};
                    unsafe{*(area.offset(194 as isize) as *mut u64x2) = v0[97]};
                    unsafe{*(area.offset(196 as isize) as *mut u64x2) = v0[98]};
                    unsafe{*(area.offset(198 as isize) as *mut u64x2) = v0[99]};
                    unsafe{*(area.offset(200 as isize) as *mut u64x2) = v0[100]};
                    unsafe{*(area.offset(202 as isize) as *mut u64x2) = v0[101]};
                    unsafe{*(area.offset(204 as isize) as *mut u64x2) = v0[102]};
                    unsafe{*(area.offset(206 as isize) as *mut u64x2) = v0[103]};
                    unsafe{*(area.offset(208 as isize) as *mut u64x2) = v0[104]};
                    unsafe{*(area.offset(210 as isize) as *mut u64x2) = v0[105]};
                    unsafe{*(area.offset(212 as isize) as *mut u64x2) = v0[106]};
                    unsafe{*(area.offset(214 as isize) as *mut u64x2) = v0[107]};
                    unsafe{*(area.offset(216 as isize) as *mut u64x2) = v0[108]};
                    unsafe{*(area.offset(218 as isize) as *mut u64x2) = v0[109]};
                    unsafe{*(area.offset(220 as isize) as *mut u64x2) = v0[110]};
                    unsafe{*(area.offset(222 as isize) as *mut u64x2) = v0[111]};
                    unsafe{*(area.offset(224 as isize) as *mut u64x2) = v0[112]};
                    unsafe{*(area.offset(226 as isize) as *mut u64x2) = v0[113]};
                    unsafe{*(area.offset(228 as isize) as *mut u64x2) = v0[114]};
                    unsafe{*(area.offset(230 as isize) as *mut u64x2) = v0[115]};
                    unsafe{*(area.offset(232 as isize) as *mut u64x2) = v0[116]};
                    unsafe{*(area.offset(234 as isize) as *mut u64x2) = v0[117]};
                    unsafe{*(area.offset(236 as isize) as *mut u64x2) = v0[118]};
                    unsafe{*(area.offset(238 as isize) as *mut u64x2) = v0[119]};
                    unsafe{*(area.offset(240 as isize) as *mut u64x2) = v0[120]};
                    unsafe{*(area.offset(242 as isize) as *mut u64x2) = v0[121]};
                    unsafe{*(area.offset(244 as isize) as *mut u64x2) = v0[122]};
                    unsafe{*(area.offset(246 as isize) as *mut u64x2) = v0[123]};
                    unsafe{*(area.offset(248 as isize) as *mut u64x2) = v0[124]};
                    unsafe{*(area.offset(250 as isize) as *mut u64x2) = v0[125]};
                    unsafe{*(area.offset(252 as isize) as *mut u64x2) = v0[126]};
                    unsafe{*(area.offset(254 as isize) as *mut u64x2) = v0[127]};
                    unsafe{*(area.offset(256 as isize) as *mut u64x2) = v0[128]};
                    unsafe{*(area.offset(258 as isize) as *mut u64x2) = v0[129]};
                    unsafe{*(area.offset(260 as isize) as *mut u64x2) = v0[130]};
                    unsafe{*(area.offset(262 as isize) as *mut u64x2) = v0[131]};
                    unsafe{*(area.offset(264 as isize) as *mut u64x2) = v0[132]};
                    unsafe{*(area.offset(266 as isize) as *mut u64x2) = v0[133]};
                    unsafe{*(area.offset(268 as isize) as *mut u64x2) = v0[134]};
                    unsafe{*(area.offset(270 as isize) as *mut u64x2) = v0[135]};
                    unsafe{*(area.offset(272 as isize) as *mut u64x2) = v0[136]};
                    unsafe{*(area.offset(274 as isize) as *mut u64x2) = v0[137]};
                    unsafe{*(area.offset(276 as isize) as *mut u64x2) = v0[138]};
                    unsafe{*(area.offset(278 as isize) as *mut u64x2) = v0[139]};
                    unsafe{*(area.offset(280 as isize) as *mut u64x2) = v0[140]};
                    unsafe{*(area.offset(282 as isize) as *mut u64x2) = v0[141]};
                    unsafe{*(area.offset(284 as isize) as *mut u64x2) = v0[142]};
                    unsafe{*(area.offset(286 as isize) as *mut u64x2) = v0[143]};
                    unsafe{*(area.offset(288 as isize) as *mut u64x2) = v0[144]};
                    unsafe{*(area.offset(290 as isize) as *mut u64x2) = v0[145]};
                    unsafe{*(area.offset(292 as isize) as *mut u64x2) = v0[146]};
                    unsafe{*(area.offset(294 as isize) as *mut u64x2) = v0[147]};
                    unsafe{*(area.offset(296 as isize) as *mut u64x2) = v0[148]};
                    unsafe{*(area.offset(298 as isize) as *mut u64x2) = v0[149]};
                    unsafe{*(area.offset(300 as isize) as *mut u64x2) = v0[150]};
                    unsafe{*(area.offset(302 as isize) as *mut u64x2) = v0[151]};
                    unsafe{*(area.offset(304 as isize) as *mut u64x2) = v0[152]};
                    unsafe{*(area.offset(306 as isize) as *mut u64x2) = v0[153]};
                    unsafe{*(area.offset(308 as isize) as *mut u64x2) = v0[154]};
                    unsafe{*(area.offset(310 as isize) as *mut u64x2) = v0[155]};
                    unsafe{*(area.offset(312 as isize) as *mut u64x2) = v0[156]};
                    unsafe{*(area.offset(314 as isize) as *mut u64x2) = v0[157]};
                    unsafe{*(area.offset(316 as isize) as *mut u64x2) = v0[158]};
                    unsafe{*(area.offset(318 as isize) as *mut u64x2) = v0[159]};
                    unsafe{*(area.offset(320 as isize) as *mut u64x2) = v0[160]};
                    unsafe{*(area.offset(322 as isize) as *mut u64x2) = v0[161]};
                    unsafe{*(area.offset(324 as isize) as *mut u64x2) = v0[162]};
                    unsafe{*(area.offset(326 as isize) as *mut u64x2) = v0[163]};
                    unsafe{*(area.offset(328 as isize) as *mut u64x2) = v0[164]};
                    unsafe{*(area.offset(330 as isize) as *mut u64x2) = v0[165]};
                    unsafe{*(area.offset(332 as isize) as *mut u64x2) = v0[166]};
                    unsafe{*(area.offset(334 as isize) as *mut u64x2) = v0[167]};
                    unsafe{*(area.offset(336 as isize) as *mut u64x2) = v0[168]};
                    unsafe{*(area.offset(338 as isize) as *mut u64x2) = v0[169]};
                    unsafe{*(area.offset(340 as isize) as *mut u64x2) = v0[170]};
                    unsafe{*(area.offset(342 as isize) as *mut u64x2) = v0[171]};
                    unsafe{*(area.offset(344 as isize) as *mut u64x2) = v0[172]};
                    unsafe{*(area.offset(346 as isize) as *mut u64x2) = v0[173]};
                    unsafe{*(area.offset(348 as isize) as *mut u64x2) = v0[174]};
                    unsafe{*(area.offset(350 as isize) as *mut u64x2) = v0[175]};
                    unsafe{*(area.offset(352 as isize) as *mut u64x2) = v0[176]};
                    unsafe{*(area.offset(354 as isize) as *mut u64x2) = v0[177]};
                    unsafe{*(area.offset(356 as isize) as *mut u64x2) = v0[178]};
                    unsafe{*(area.offset(358 as isize) as *mut u64x2) = v0[179]};
                    unsafe{*(area.offset(360 as isize) as *mut u64x2) = v0[180]};
                    unsafe{*(area.offset(362 as isize) as *mut u64x2) = v0[181]};
                    unsafe{*(area.offset(364 as isize) as *mut u64x2) = v0[182]};
                    unsafe{*(area.offset(366 as isize) as *mut u64x2) = v0[183]};
                    unsafe{*(area.offset(368 as isize) as *mut u64x2) = v0[184]};
                    unsafe{*(area.offset(370 as isize) as *mut u64x2) = v0[185]};
                    unsafe{*(area.offset(372 as isize) as *mut u64x2) = v0[186]};
                    unsafe{*(area.offset(374 as isize) as *mut u64x2) = v0[187]};
                    unsafe{*(area.offset(376 as isize) as *mut u64x2) = v0[188]};
                    unsafe{*(area.offset(378 as isize) as *mut u64x2) = v0[189]};
                    unsafe{*(area.offset(380 as isize) as *mut u64x2) = v0[190]};
                    unsafe{*(area.offset(382 as isize) as *mut u64x2) = v0[191]};
                    unsafe{*(area.offset(384 as isize) as *mut u64x2) = v0[192]};
                    unsafe{*(area.offset(386 as isize) as *mut u64x2) = v0[193]};
                    unsafe{*(area.offset(388 as isize) as *mut u64x2) = v0[194]};
                    unsafe{*(area.offset(390 as isize) as *mut u64x2) = v0[195]};
                    unsafe{*(area.offset(392 as isize) as *mut u64x2) = v0[196]};
                    unsafe{*(area.offset(394 as isize) as *mut u64x2) = v0[197]};
                    unsafe{*(area.offset(396 as isize) as *mut u64x2) = v0[198]};
                    unsafe{*(area.offset(398 as isize) as *mut u64x2) = v0[199]};
                    unsafe{*(area.offset(400 as isize) as *mut u64x2) = v0[200]};
                    unsafe{*(area.offset(402 as isize) as *mut u64x2) = v0[201]};
                    unsafe{*(area.offset(404 as isize) as *mut u64x2) = v0[202]};
                    unsafe{*(area.offset(406 as isize) as *mut u64x2) = v0[203]};
                    unsafe{*(area.offset(408 as isize) as *mut u64x2) = v0[204]};
                    unsafe{*(area.offset(410 as isize) as *mut u64x2) = v0[205]};
                    unsafe{*(area.offset(412 as isize) as *mut u64x2) = v0[206]};
                    unsafe{*(area.offset(414 as isize) as *mut u64x2) = v0[207]};
                    unsafe{*(area.offset(416 as isize) as *mut u64x2) = v0[208]};
                    unsafe{*(area.offset(418 as isize) as *mut u64x2) = v0[209]};
                    unsafe{*(area.offset(420 as isize) as *mut u64x2) = v0[210]};
                    unsafe{*(area.offset(422 as isize) as *mut u64x2) = v0[211]};
                    unsafe{*(area.offset(424 as isize) as *mut u64x2) = v0[212]};
                    unsafe{*(area.offset(426 as isize) as *mut u64x2) = v0[213]};
                    unsafe{*(area.offset(428 as isize) as *mut u64x2) = v0[214]};
                    unsafe{*(area.offset(430 as isize) as *mut u64x2) = v0[215]};
                    unsafe{*(area.offset(432 as isize) as *mut u64x2) = v0[216]};
                    unsafe{*(area.offset(434 as isize) as *mut u64x2) = v0[217]};
                    unsafe{*(area.offset(436 as isize) as *mut u64x2) = v0[218]};
                    unsafe{*(area.offset(438 as isize) as *mut u64x2) = v0[219]};
                    unsafe{*(area.offset(440 as isize) as *mut u64x2) = v0[220]};
                    unsafe{*(area.offset(442 as isize) as *mut u64x2) = v0[221]};
                    unsafe{*(area.offset(444 as isize) as *mut u64x2) = v0[222]};
                    unsafe{*(area.offset(446 as isize) as *mut u64x2) = v0[223]};
                    unsafe{*(area.offset(448 as isize) as *mut u64x2) = v0[224]};
                    unsafe{*(area.offset(450 as isize) as *mut u64x2) = v0[225]};
                    unsafe{*(area.offset(452 as isize) as *mut u64x2) = v0[226]};
                    unsafe{*(area.offset(454 as isize) as *mut u64x2) = v0[227]};
                    unsafe{*(area.offset(456 as isize) as *mut u64x2) = v0[228]};
                    unsafe{*(area.offset(458 as isize) as *mut u64x2) = v0[229]};
                    unsafe{*(area.offset(460 as isize) as *mut u64x2) = v0[230]};
                    unsafe{*(area.offset(462 as isize) as *mut u64x2) = v0[231]};
                    unsafe{*(area.offset(464 as isize) as *mut u64x2) = v0[232]};
                    unsafe{*(area.offset(466 as isize) as *mut u64x2) = v0[233]};
                    unsafe{*(area.offset(468 as isize) as *mut u64x2) = v0[234]};
                    unsafe{*(area.offset(470 as isize) as *mut u64x2) = v0[235]};
                    unsafe{*(area.offset(472 as isize) as *mut u64x2) = v0[236]};
                    unsafe{*(area.offset(474 as isize) as *mut u64x2) = v0[237]};
                    unsafe{*(area.offset(476 as isize) as *mut u64x2) = v0[238]};
                    unsafe{*(area.offset(478 as isize) as *mut u64x2) = v0[239]};
                    unsafe{*(area.offset(480 as isize) as *mut u64x2) = v0[240]};
                    unsafe{*(area.offset(482 as isize) as *mut u64x2) = v0[241]};
                    unsafe{*(area.offset(484 as isize) as *mut u64x2) = v0[242]};
                    unsafe{*(area.offset(486 as isize) as *mut u64x2) = v0[243]};
                    unsafe{*(area.offset(488 as isize) as *mut u64x2) = v0[244]};
                    unsafe{*(area.offset(490 as isize) as *mut u64x2) = v0[245]};
                    unsafe{*(area.offset(492 as isize) as *mut u64x2) = v0[246]};
                    unsafe{*(area.offset(494 as isize) as *mut u64x2) = v0[247]};
                    unsafe{*(area.offset(496 as isize) as *mut u64x2) = v0[248]};
                    unsafe{*(area.offset(498 as isize) as *mut u64x2) = v0[249]};
                    unsafe{*(area.offset(500 as isize) as *mut u64x2) = v0[250]};
                    unsafe{*(area.offset(502 as isize) as *mut u64x2) = v0[251]};
                    unsafe{*(area.offset(504 as isize) as *mut u64x2) = v0[252]};
                    unsafe{*(area.offset(506 as isize) as *mut u64x2) = v0[253]};
                    unsafe{*(area.offset(508 as isize) as *mut u64x2) = v0[254]};
                    unsafe{*(area.offset(510 as isize) as *mut u64x2) = v0[255]};
                    unsafe{*(area.offset(512 as isize) as *mut u64x2) = v0[256]};
                    unsafe{*(area.offset(514 as isize) as *mut u64x2) = v0[257]};
                    unsafe{*(area.offset(516 as isize) as *mut u64x2) = v0[258]};
                    unsafe{*(area.offset(518 as isize) as *mut u64x2) = v0[259]};
                    unsafe{*(area.offset(520 as isize) as *mut u64x2) = v0[260]};
                    unsafe{*(area.offset(522 as isize) as *mut u64x2) = v0[261]};
                    unsafe{*(area.offset(524 as isize) as *mut u64x2) = v0[262]};
                    unsafe{*(area.offset(526 as isize) as *mut u64x2) = v0[263]};
                    unsafe{*(area.offset(528 as isize) as *mut u64x2) = v0[264]};
                    unsafe{*(area.offset(530 as isize) as *mut u64x2) = v0[265]};
                    unsafe{*(area.offset(532 as isize) as *mut u64x2) = v0[266]};
                    unsafe{*(area.offset(534 as isize) as *mut u64x2) = v0[267]};
                    unsafe{*(area.offset(536 as isize) as *mut u64x2) = v0[268]};
                    unsafe{*(area.offset(538 as isize) as *mut u64x2) = v0[269]};
                    unsafe{*(area.offset(540 as isize) as *mut u64x2) = v0[270]};
                    unsafe{*(area.offset(542 as isize) as *mut u64x2) = v0[271]};
                    unsafe{*(area.offset(544 as isize) as *mut u64x2) = v0[272]};
                    unsafe{*(area.offset(546 as isize) as *mut u64x2) = v0[273]};
                    unsafe{*(area.offset(548 as isize) as *mut u64x2) = v0[274]};
                    unsafe{*(area.offset(550 as isize) as *mut u64x2) = v0[275]};
                    unsafe{*(area.offset(552 as isize) as *mut u64x2) = v0[276]};
                    unsafe{*(area.offset(554 as isize) as *mut u64x2) = v0[277]};
                    unsafe{*(area.offset(556 as isize) as *mut u64x2) = v0[278]};
                    unsafe{*(area.offset(558 as isize) as *mut u64x2) = v0[279]};
                    unsafe{*(area.offset(560 as isize) as *mut u64x2) = v0[280]};
                    unsafe{*(area.offset(562 as isize) as *mut u64x2) = v0[281]};
                    unsafe{*(area.offset(564 as isize) as *mut u64x2) = v0[282]};
                    unsafe{*(area.offset(566 as isize) as *mut u64x2) = v0[283]};
                    unsafe{*(area.offset(568 as isize) as *mut u64x2) = v0[284]};
                    unsafe{*(area.offset(570 as isize) as *mut u64x2) = v0[285]};
                    unsafe{*(area.offset(572 as isize) as *mut u64x2) = v0[286]};
                    unsafe{*(area.offset(574 as isize) as *mut u64x2) = v0[287]};
                    unsafe{*(area.offset(576 as isize) as *mut u64x2) = v0[288]};
                    unsafe{*(area.offset(578 as isize) as *mut u64x2) = v0[289]};
                    unsafe{*(area.offset(580 as isize) as *mut u64x2) = v0[290]};
                    unsafe{*(area.offset(582 as isize) as *mut u64x2) = v0[291]};
                    unsafe{*(area.offset(584 as isize) as *mut u64x2) = v0[292]};
                    unsafe{*(area.offset(586 as isize) as *mut u64x2) = v0[293]};
                    unsafe{*(area.offset(588 as isize) as *mut u64x2) = v0[294]};
                    unsafe{*(area.offset(590 as isize) as *mut u64x2) = v0[295]};
                    unsafe{*(area.offset(592 as isize) as *mut u64x2) = v0[296]};
                    unsafe{*(area.offset(594 as isize) as *mut u64x2) = v0[297]};
                    unsafe{*(area.offset(596 as isize) as *mut u64x2) = v0[298]};
                    unsafe{*(area.offset(598 as isize) as *mut u64x2) = v0[299]};
                    unsafe{*(area.offset(600 as isize) as *mut u64x2) = v0[300]};
                    unsafe{*(area.offset(602 as isize) as *mut u64x2) = v0[301]};
                    unsafe{*(area.offset(604 as isize) as *mut u64x2) = v0[302]};
                    unsafe{*(area.offset(606 as isize) as *mut u64x2) = v0[303]};
                    unsafe{*(area.offset(608 as isize) as *mut u64x2) = v0[304]};
                    unsafe{*(area.offset(610 as isize) as *mut u64x2) = v0[305]};
                    unsafe{*(area.offset(612 as isize) as *mut u64x2) = v0[306]};
                    unsafe{*(area.offset(614 as isize) as *mut u64x2) = v0[307]};
                    unsafe{*(area.offset(616 as isize) as *mut u64x2) = v0[308]};
                    unsafe{*(area.offset(618 as isize) as *mut u64x2) = v0[309]};
                    unsafe{*(area.offset(620 as isize) as *mut u64x2) = v0[310]};
                    unsafe{*(area.offset(622 as isize) as *mut u64x2) = v0[311]};
                    unsafe{*(area.offset(624 as isize) as *mut u64x2) = v0[312]};
                    unsafe{*(area.offset(626 as isize) as *mut u64x2) = v0[313]};
                    unsafe{*(area.offset(628 as isize) as *mut u64x2) = v0[314]};
                    unsafe{*(area.offset(630 as isize) as *mut u64x2) = v0[315]};
                    unsafe{*(area.offset(632 as isize) as *mut u64x2) = v0[316]};
                    unsafe{*(area.offset(634 as isize) as *mut u64x2) = v0[317]};
                    unsafe{*(area.offset(636 as isize) as *mut u64x2) = v0[318]};
                    unsafe{*(area.offset(638 as isize) as *mut u64x2) = v0[319]};
                    unsafe{*(area.offset(640 as isize) as *mut u64x2) = v0[320]};
                    unsafe{*(area.offset(642 as isize) as *mut u64x2) = v0[321]};
                    unsafe{*(area.offset(644 as isize) as *mut u64x2) = v0[322]};
                    unsafe{*(area.offset(646 as isize) as *mut u64x2) = v0[323]};
                    unsafe{*(area.offset(648 as isize) as *mut u64x2) = v0[324]};
                    unsafe{*(area.offset(650 as isize) as *mut u64x2) = v0[325]};
                    unsafe{*(area.offset(652 as isize) as *mut u64x2) = v0[326]};
                    unsafe{*(area.offset(654 as isize) as *mut u64x2) = v0[327]};
                    unsafe{*(area.offset(656 as isize) as *mut u64x2) = v0[328]};
                    unsafe{*(area.offset(658 as isize) as *mut u64x2) = v0[329]};
                    unsafe{*(area.offset(660 as isize) as *mut u64x2) = v0[330]};
                    unsafe{*(area.offset(662 as isize) as *mut u64x2) = v0[331]};
                    unsafe{*(area.offset(664 as isize) as *mut u64x2) = v0[332]};
                    unsafe{*(area.offset(666 as isize) as *mut u64x2) = v0[333]};
                    unsafe{*(area.offset(668 as isize) as *mut u64x2) = v0[334]};
                    unsafe{*(area.offset(670 as isize) as *mut u64x2) = v0[335]};
                    unsafe{*(area.offset(672 as isize) as *mut u64x2) = v0[336]};
                    unsafe{*(area.offset(674 as isize) as *mut u64x2) = v0[337]};
                    unsafe{*(area.offset(676 as isize) as *mut u64x2) = v0[338]};
                    unsafe{*(area.offset(678 as isize) as *mut u64x2) = v0[339]};
                    unsafe{*(area.offset(680 as isize) as *mut u64x2) = v0[340]};
                    unsafe{*(area.offset(682 as isize) as *mut u64x2) = v0[341]};
                    unsafe{*(area.offset(684 as isize) as *mut u64x2) = v0[342]};
                    unsafe{*(area.offset(686 as isize) as *mut u64x2) = v0[343]};
                    unsafe{*(area.offset(688 as isize) as *mut u64x2) = v0[344]};
                    unsafe{*(area.offset(690 as isize) as *mut u64x2) = v0[345]};
                    unsafe{*(area.offset(692 as isize) as *mut u64x2) = v0[346]};
                    unsafe{*(area.offset(694 as isize) as *mut u64x2) = v0[347]};
                    unsafe{*(area.offset(696 as isize) as *mut u64x2) = v0[348]};
                    unsafe{*(area.offset(698 as isize) as *mut u64x2) = v0[349]};
                    unsafe{*(area.offset(700 as isize) as *mut u64x2) = v0[350]};
                    unsafe{*(area.offset(702 as isize) as *mut u64x2) = v0[351]};
                    unsafe{*(area.offset(704 as isize) as *mut u64x2) = v0[352]};
                    unsafe{*(area.offset(706 as isize) as *mut u64x2) = v0[353]};
                    unsafe{*(area.offset(708 as isize) as *mut u64x2) = v0[354]};
                    unsafe{*(area.offset(710 as isize) as *mut u64x2) = v0[355]};
                    unsafe{*(area.offset(712 as isize) as *mut u64x2) = v0[356]};
                    unsafe{*(area.offset(714 as isize) as *mut u64x2) = v0[357]};
                    unsafe{*(area.offset(716 as isize) as *mut u64x2) = v0[358]};
                    unsafe{*(area.offset(718 as isize) as *mut u64x2) = v0[359]};
                    unsafe{*(area.offset(720 as isize) as *mut u64x2) = v0[360]};
                    unsafe{*(area.offset(722 as isize) as *mut u64x2) = v0[361]};
                    unsafe{*(area.offset(724 as isize) as *mut u64x2) = v0[362]};
                    unsafe{*(area.offset(726 as isize) as *mut u64x2) = v0[363]};
                    unsafe{*(area.offset(728 as isize) as *mut u64x2) = v0[364]};
                    unsafe{*(area.offset(730 as isize) as *mut u64x2) = v0[365]};
                    unsafe{*(area.offset(732 as isize) as *mut u64x2) = v0[366]};
                    unsafe{*(area.offset(734 as isize) as *mut u64x2) = v0[367]};
                    unsafe{*(area.offset(736 as isize) as *mut u64x2) = v0[368]};
                    unsafe{*(area.offset(738 as isize) as *mut u64x2) = v0[369]};
                    unsafe{*(area.offset(740 as isize) as *mut u64x2) = v0[370]};
                    unsafe{*(area.offset(742 as isize) as *mut u64x2) = v0[371]};
                    unsafe{*(area.offset(744 as isize) as *mut u64x2) = v0[372]};
                    unsafe{*(area.offset(746 as isize) as *mut u64x2) = v0[373]};
                    unsafe{*(area.offset(748 as isize) as *mut u64x2) = v0[374]};
                    unsafe{*(area.offset(750 as isize) as *mut u64x2) = v0[375]};
                    unsafe{*(area.offset(752 as isize) as *mut u64x2) = v0[376]};
                    unsafe{*(area.offset(754 as isize) as *mut u64x2) = v0[377]};
                    unsafe{*(area.offset(756 as isize) as *mut u64x2) = v0[378]};
                    unsafe{*(area.offset(758 as isize) as *mut u64x2) = v0[379]};
                    unsafe{*(area.offset(760 as isize) as *mut u64x2) = v0[380]};
                    unsafe{*(area.offset(762 as isize) as *mut u64x2) = v0[381]};
                    unsafe{*(area.offset(764 as isize) as *mut u64x2) = v0[382]};
                    unsafe{*(area.offset(766 as isize) as *mut u64x2) = v0[383]};
                    unsafe{*(area.offset(768 as isize) as *mut u64x2) = v0[384]};
                    unsafe{*(area.offset(770 as isize) as *mut u64x2) = v0[385]};
                    unsafe{*(area.offset(772 as isize) as *mut u64x2) = v0[386]};
                    unsafe{*(area.offset(774 as isize) as *mut u64x2) = v0[387]};
                    unsafe{*(area.offset(776 as isize) as *mut u64x2) = v0[388]};
                    unsafe{*(area.offset(778 as isize) as *mut u64x2) = v0[389]};
                    unsafe{*(area.offset(780 as isize) as *mut u64x2) = v0[390]};
                    unsafe{*(area.offset(782 as isize) as *mut u64x2) = v0[391]};
                    unsafe{*(area.offset(784 as isize) as *mut u64x2) = v0[392]};
                    unsafe{*(area.offset(786 as isize) as *mut u64x2) = v0[393]};
                    unsafe{*(area.offset(788 as isize) as *mut u64x2) = v0[394]};
                    unsafe{*(area.offset(790 as isize) as *mut u64x2) = v0[395]};
                    unsafe{*(area.offset(792 as isize) as *mut u64x2) = v0[396]};
                    unsafe{*(area.offset(794 as isize) as *mut u64x2) = v0[397]};
                    unsafe{*(area.offset(796 as isize) as *mut u64x2) = v0[398]};
                    unsafe{*(area.offset(798 as isize) as *mut u64x2) = v0[399]};
                    unsafe{*(area.offset(800 as isize) as *mut u64x2) = v0[400]};
                    unsafe{*(area.offset(802 as isize) as *mut u64x2) = v0[401]};
                    unsafe{*(area.offset(804 as isize) as *mut u64x2) = v0[402]};
                    unsafe{*(area.offset(806 as isize) as *mut u64x2) = v0[403]};
                    unsafe{*(area.offset(808 as isize) as *mut u64x2) = v0[404]};
                    unsafe{*(area.offset(810 as isize) as *mut u64x2) = v0[405]};
                    unsafe{*(area.offset(812 as isize) as *mut u64x2) = v0[406]};
                    unsafe{*(area.offset(814 as isize) as *mut u64x2) = v0[407]};
                    unsafe{*(area.offset(816 as isize) as *mut u64x2) = v0[408]};
                    unsafe{*(area.offset(818 as isize) as *mut u64x2) = v0[409]};
                    unsafe{*(area.offset(820 as isize) as *mut u64x2) = v0[410]};
                    unsafe{*(area.offset(822 as isize) as *mut u64x2) = v0[411]};
                    unsafe{*(area.offset(824 as isize) as *mut u64x2) = v0[412]};
                    unsafe{*(area.offset(826 as isize) as *mut u64x2) = v0[413]};
                    unsafe{*(area.offset(828 as isize) as *mut u64x2) = v0[414]};
                    unsafe{*(area.offset(830 as isize) as *mut u64x2) = v0[415]};
                    unsafe{*(area.offset(832 as isize) as *mut u64x2) = v0[416]};
                    unsafe{*(area.offset(834 as isize) as *mut u64x2) = v0[417]};
                    unsafe{*(area.offset(836 as isize) as *mut u64x2) = v0[418]};
                    unsafe{*(area.offset(838 as isize) as *mut u64x2) = v0[419]};
                    unsafe{*(area.offset(840 as isize) as *mut u64x2) = v0[420]};
                    unsafe{*(area.offset(842 as isize) as *mut u64x2) = v0[421]};
                    unsafe{*(area.offset(844 as isize) as *mut u64x2) = v0[422]};
                    unsafe{*(area.offset(846 as isize) as *mut u64x2) = v0[423]};
                    unsafe{*(area.offset(848 as isize) as *mut u64x2) = v0[424]};
                    unsafe{*(area.offset(850 as isize) as *mut u64x2) = v0[425]};
                    unsafe{*(area.offset(852 as isize) as *mut u64x2) = v0[426]};
                    unsafe{*(area.offset(854 as isize) as *mut u64x2) = v0[427]};
                    unsafe{*(area.offset(856 as isize) as *mut u64x2) = v0[428]};
                    unsafe{*(area.offset(858 as isize) as *mut u64x2) = v0[429]};
                    unsafe{*(area.offset(860 as isize) as *mut u64x2) = v0[430]};
                    unsafe{*(area.offset(862 as isize) as *mut u64x2) = v0[431]};
                    unsafe{*(area.offset(864 as isize) as *mut u64x2) = v0[432]};
                    unsafe{*(area.offset(866 as isize) as *mut u64x2) = v0[433]};
                    unsafe{*(area.offset(868 as isize) as *mut u64x2) = v0[434]};
                    unsafe{*(area.offset(870 as isize) as *mut u64x2) = v0[435]};
                    unsafe{*(area.offset(872 as isize) as *mut u64x2) = v0[436]};
                    unsafe{*(area.offset(874 as isize) as *mut u64x2) = v0[437]};
                    unsafe{*(area.offset(876 as isize) as *mut u64x2) = v0[438]};
                    unsafe{*(area.offset(878 as isize) as *mut u64x2) = v0[439]};
                    unsafe{*(area.offset(880 as isize) as *mut u64x2) = v0[440]};
                    unsafe{*(area.offset(882 as isize) as *mut u64x2) = v0[441]};
                    unsafe{*(area.offset(884 as isize) as *mut u64x2) = v0[442]};
                    unsafe{*(area.offset(886 as isize) as *mut u64x2) = v0[443]};
                    unsafe{*(area.offset(888 as isize) as *mut u64x2) = v0[444]};
                    unsafe{*(area.offset(890 as isize) as *mut u64x2) = v0[445]};
                    unsafe{*(area.offset(892 as isize) as *mut u64x2) = v0[446]};
                    unsafe{*(area.offset(894 as isize) as *mut u64x2) = v0[447]};
                    unsafe{*(area.offset(896 as isize) as *mut u64x2) = v0[448]};
                    unsafe{*(area.offset(898 as isize) as *mut u64x2) = v0[449]};
                    unsafe{*(area.offset(900 as isize) as *mut u64x2) = v0[450]};
                    unsafe{*(area.offset(902 as isize) as *mut u64x2) = v0[451]};
                    unsafe{*(area.offset(904 as isize) as *mut u64x2) = v0[452]};
                    unsafe{*(area.offset(906 as isize) as *mut u64x2) = v0[453]};
                    unsafe{*(area.offset(908 as isize) as *mut u64x2) = v0[454]};
                    unsafe{*(area.offset(910 as isize) as *mut u64x2) = v0[455]};
                    unsafe{*(area.offset(912 as isize) as *mut u64x2) = v0[456]};
                    unsafe{*(area.offset(914 as isize) as *mut u64x2) = v0[457]};
                    unsafe{*(area.offset(916 as isize) as *mut u64x2) = v0[458]};
                    unsafe{*(area.offset(918 as isize) as *mut u64x2) = v0[459]};
                    unsafe{*(area.offset(920 as isize) as *mut u64x2) = v0[460]};
                    unsafe{*(area.offset(922 as isize) as *mut u64x2) = v0[461]};
                    unsafe{*(area.offset(924 as isize) as *mut u64x2) = v0[462]};
                    unsafe{*(area.offset(926 as isize) as *mut u64x2) = v0[463]};
                    unsafe{*(area.offset(928 as isize) as *mut u64x2) = v0[464]};
                    unsafe{*(area.offset(930 as isize) as *mut u64x2) = v0[465]};
                    unsafe{*(area.offset(932 as isize) as *mut u64x2) = v0[466]};
                    unsafe{*(area.offset(934 as isize) as *mut u64x2) = v0[467]};
                    unsafe{*(area.offset(936 as isize) as *mut u64x2) = v0[468]};
                    unsafe{*(area.offset(938 as isize) as *mut u64x2) = v0[469]};
                    unsafe{*(area.offset(940 as isize) as *mut u64x2) = v0[470]};
                    unsafe{*(area.offset(942 as isize) as *mut u64x2) = v0[471]};
                    unsafe{*(area.offset(944 as isize) as *mut u64x2) = v0[472]};
                    unsafe{*(area.offset(946 as isize) as *mut u64x2) = v0[473]};
                    unsafe{*(area.offset(948 as isize) as *mut u64x2) = v0[474]};
                    unsafe{*(area.offset(950 as isize) as *mut u64x2) = v0[475]};
                    unsafe{*(area.offset(952 as isize) as *mut u64x2) = v0[476]};
                    unsafe{*(area.offset(954 as isize) as *mut u64x2) = v0[477]};
                    unsafe{*(area.offset(956 as isize) as *mut u64x2) = v0[478]};
                    unsafe{*(area.offset(958 as isize) as *mut u64x2) = v0[479]};
                    unsafe{*(area.offset(960 as isize) as *mut u64x2) = v0[480]};
                    unsafe{*(area.offset(962 as isize) as *mut u64x2) = v0[481]};
                    unsafe{*(area.offset(964 as isize) as *mut u64x2) = v0[482]};
                    unsafe{*(area.offset(966 as isize) as *mut u64x2) = v0[483]};
                    unsafe{*(area.offset(968 as isize) as *mut u64x2) = v0[484]};
                    unsafe{*(area.offset(970 as isize) as *mut u64x2) = v0[485]};
                    unsafe{*(area.offset(972 as isize) as *mut u64x2) = v0[486]};
                    unsafe{*(area.offset(974 as isize) as *mut u64x2) = v0[487]};
                    unsafe{*(area.offset(976 as isize) as *mut u64x2) = v0[488]};
                    unsafe{*(area.offset(978 as isize) as *mut u64x2) = v0[489]};
                    unsafe{*(area.offset(980 as isize) as *mut u64x2) = v0[490]};
                    unsafe{*(area.offset(982 as isize) as *mut u64x2) = v0[491]};
                    unsafe{*(area.offset(984 as isize) as *mut u64x2) = v0[492]};
                    unsafe{*(area.offset(986 as isize) as *mut u64x2) = v0[493]};
                    unsafe{*(area.offset(988 as isize) as *mut u64x2) = v0[494]};
                    unsafe{*(area.offset(990 as isize) as *mut u64x2) = v0[495]};
                    unsafe{*(area.offset(992 as isize) as *mut u64x2) = v0[496]};
                    unsafe{*(area.offset(994 as isize) as *mut u64x2) = v0[497]};
                    unsafe{*(area.offset(996 as isize) as *mut u64x2) = v0[498]};
                    unsafe{*(area.offset(998 as isize) as *mut u64x2) = v0[499]};
                    unsafe{*(area.offset(1000 as isize) as *mut u64x2) = v0[500]};
                    unsafe{*(area.offset(1002 as isize) as *mut u64x2) = v0[501]};
                    unsafe{*(area.offset(1004 as isize) as *mut u64x2) = v0[502]};
                    unsafe{*(area.offset(1006 as isize) as *mut u64x2) = v0[503]};
                    unsafe{*(area.offset(1008 as isize) as *mut u64x2) = v0[504]};
                    unsafe{*(area.offset(1010 as isize) as *mut u64x2) = v0[505]};
                    unsafe{*(area.offset(1012 as isize) as *mut u64x2) = v0[506]};
                    unsafe{*(area.offset(1014 as isize) as *mut u64x2) = v0[507]};
                    unsafe{*(area.offset(1016 as isize) as *mut u64x2) = v0[508]};
                    unsafe{*(area.offset(1018 as isize) as *mut u64x2) = v0[509]};
                    unsafe{*(area.offset(1020 as isize) as *mut u64x2) = v0[510]};
                    unsafe{*(area.offset(1022 as isize) as *mut u64x2) = v0[511]};
                    unsafe{*(area.offset(1024 as isize) as *mut u64x2) = v0[512]};
                    unsafe{*(area.offset(1026 as isize) as *mut u64x2) = v0[513]};
                    unsafe{*(area.offset(1028 as isize) as *mut u64x2) = v0[514]};
                    unsafe{*(area.offset(1030 as isize) as *mut u64x2) = v0[515]};
                    unsafe{*(area.offset(1032 as isize) as *mut u64x2) = v0[516]};
                    unsafe{*(area.offset(1034 as isize) as *mut u64x2) = v0[517]};
                    unsafe{*(area.offset(1036 as isize) as *mut u64x2) = v0[518]};
                    unsafe{*(area.offset(1038 as isize) as *mut u64x2) = v0[519]};
                    unsafe{*(area.offset(1040 as isize) as *mut u64x2) = v0[520]};
                    unsafe{*(area.offset(1042 as isize) as *mut u64x2) = v0[521]};
                    unsafe{*(area.offset(1044 as isize) as *mut u64x2) = v0[522]};
                    unsafe{*(area.offset(1046 as isize) as *mut u64x2) = v0[523]};
                    unsafe{*(area.offset(1048 as isize) as *mut u64x2) = v0[524]};
                    unsafe{*(area.offset(1050 as isize) as *mut u64x2) = v0[525]};
                    unsafe{*(area.offset(1052 as isize) as *mut u64x2) = v0[526]};
                    unsafe{*(area.offset(1054 as isize) as *mut u64x2) = v0[527]};
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
                for i in (1024..139).step_by(2) {
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
