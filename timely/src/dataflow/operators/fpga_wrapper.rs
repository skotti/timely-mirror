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

use crate::progress::frontier::MutableAntichain;
use std::cell::RefCell;
use std::convert::TryInto;
use std::rc::Rc;

use std::ptr;

use std::collections::HashMap;
use std::ffi::c_void;

// Various parameters
const NUMBER_OF_INPUTS: usize = 16; // make sure to sync with caller (e.g. `hello_fpga.rs`)
const NUMBER_OF_FILTER_OPERATORS: usize = 1;
const NUMBER_OF_MAP_OPERATORS: usize = 0;
const OPERATOR_COUNT: usize = NUMBER_OF_FILTER_OPERATORS + NUMBER_OF_MAP_OPERATORS;
const PARAM: usize = 2;
const PARAM_OUTPUT: usize = 2;
const FRONTIER_PARAM: usize = 3;
const FRONTIER_LENGTH: usize = FRONTIER_PARAM * 8;
const MAX_LENGTH: usize = PARAM * 8 + FRONTIER_PARAM * 8;
const DATA_LENGTH: usize = PARAM_OUTPUT * 8;
const PROGRESS_START_INDEX: usize = PARAM_OUTPUT * 8;

#[derive(Debug)]
#[repr(C)]
/// Data structure to store FPGA related data
pub struct HardwareCommon {
    /// Input memory
    pub h_mem: *mut c_void,
    /// Output memory
    pub o_mem: *mut c_void,
    /// the mmapped cache lines
    pub area: *mut c_void,
}

unsafe impl Send for HardwareCommon {}
unsafe impl Sync for HardwareCommon {}

/// Writes a specific hardcoded bit pattern to simulate FPGA output
fn generate_fpga_output(input_ptr: *mut u64, output_ptr: *mut u64) {
    // Cast input buffer ptr to array
    let input_arr = unsafe { std::slice::from_raw_parts(input_ptr, 144) };
    let mut offset = 0; // Keep track while iterate through array

    // dbg!(input_arr[0]); // <--- iteration count
    offset += 1;

    //
    let same_value = input_arr[offset];
    for i in 0..OPERATOR_COUNT {
        let i = i + offset;
        assert!(0 == input_arr[i] || input_arr[i] % 2 == 1);
        assert_eq!(same_value, input_arr[i]); // values should be the same across
    }
    offset += OPERATOR_COUNT;

    // Safety check, otherwise we overwrite values
    assert!(offset <= FRONTIER_LENGTH);

    // Set offset to past frontier end, begining of data section
    offset = FRONTIER_LENGTH;

    //
    let same_value = input_arr[offset];
    for i in 0..NUMBER_OF_INPUTS {
        let i = i + offset;
        assert!(0 == input_arr[i] || input_arr[i] % 2 == 1);
        assert_eq!(same_value, input_arr[i]); // values should be the same across
    }

    //
    let first_val: u64;
    let second_val: u64;
    if same_value != 0 {
        first_val = 1;
        second_val = NUMBER_OF_INPUTS.try_into().unwrap();
    } else {
        first_val = 0;
        second_val = 0;
    }
    // Cast buffer ptr to array
    let output_arr: &mut [u64] = unsafe { std::slice::from_raw_parts_mut(output_ptr, 144) };
    let mut my_offset = 0;
    // 1...1 - number of inputs times
    for _i in 0..NUMBER_OF_INPUTS {
        output_arr[my_offset] = first_val;
        my_offset += 1;
    }

    // 1100 - operator many times
    for _i in 0..OPERATOR_COUNT {
        output_arr[my_offset + 0] = second_val;
        output_arr[my_offset + 1] = second_val;
        output_arr[my_offset + 2] = 0;
        output_arr[my_offset + 3] = 0;

        my_offset += 4;
    }
}

/// Debug function to read the written to memory area
fn read_hc_u64(hc: *const HardwareCommon) {
    dbg!("read_hc_u64");

    print!("o_mem: ");
    let o_mem_ptr: *mut u64 = unsafe { (*hc).o_mem as *mut u64 };
    read_memory_chunk_u64(o_mem_ptr);

    print!("h_mem: ");
    let h_mem_ptr: *mut u64 = unsafe { (*hc).h_mem as *mut u64 };
    read_memory_chunk_u64(h_mem_ptr);
}

/// Reads a chunk of memory as array of `u64`
fn read_memory_chunk_u64(dst_ptr: *mut u64) {
    let entries = 144;
    for i in 0..entries {
        let res = unsafe { ptr::read(dst_ptr.offset(i)) };
        print!("{res} ");
    }
    println!();
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

/// Writes frontiers to first cache line
fn write_frontiers(val: &[u64], area: *mut std::ffi::c_void) {
    // Treat as `uint64_t *`
    let area = area as *mut u64;

    for i in 0..16 as usize {
        unsafe { *area.offset(i.try_into().unwrap()) = val[i] };
    }
    dmb();
}
/// Writes data to second cache line
fn write_data(val: &[u64], area: *mut std::ffi::c_void) {
    // Treat as `uint64_t *`
    let area = area as *mut u64;

    for i in 0..16 as usize {
        unsafe { *area.offset((16 + i).try_into().unwrap()) = val[i] };
    }
    dmb();
}

/// Read results from the two cache lines
fn read_from_memory(area: *mut std::ffi::c_void) -> ([u64; 16], [u64; 16]) {
    let mut cache_line_1: [u64; 16] = [0; 16];
    let mut cache_line_2: [u64; 16] = [0; 16];

    // Treat as `uint64_t *`
    let area = area as *mut u64;

    // Read
    for i in 0..16 {
        cache_line_1[i] = unsafe { *(area.offset(i.try_into().unwrap())) };
    }
    for i in 0..16 {
        cache_line_2[i] = unsafe { *(area.offset((16 + i).try_into().unwrap())) };
    }
    dmb();

    (cache_line_1, cache_line_2)
}

/// Communicates to FPGA via cache lines using [`2fast2forward`](https://gitlab.inf.ethz.ch/PROJECT-Enzian/fpga-sources/enzian-applications/2fast2forward)
fn fpga_communication(hc: *const HardwareCommon, h_mem_ptr: *mut u64, o_mem_ptr: *mut u64) {
    let input_arr = unsafe { std::slice::from_raw_parts(h_mem_ptr, 144) };
    let output_arr = unsafe { std::slice::from_raw_parts_mut(o_mem_ptr, 144) };
    let val: &[u64] = &input_arr[FRONTIER_LENGTH..FRONTIER_LENGTH + 16];
    let frontiers: &[u64] = &input_arr[1..1 + 16];

    // Get pointer to memory
    let area = unsafe { (*hc).area };

    // Write to cache lines
    write_frontiers(frontiers, area);
    write_data(val, area);

    // Read results from cache lines
    let (line_1, line_2) = read_from_memory(area);

    // Copy results to output arrays
    for i in 0..line_1.len() {
        output_arr[i] = line_1[i];
    }
    for i in 0..line_2.len() {
        output_arr[16 + i] = line_2[i];
    }
}

/// Sends data to FPGA and receives reponse
fn run(hc: *const HardwareCommon) {
    let h_mem_ptr: *mut u64 = unsafe { (*hc).h_mem } as *mut u64;
    let o_mem_ptr: *mut u64 = unsafe { (*hc).o_mem } as *mut u64;

    // Only run when `no-fpga` feature is used
    #[cfg(feature = "no-fpga")]
    generate_fpga_output(h_mem_ptr, o_mem_ptr);

    // Only run when using FPGA
    #[cfg(not(feature = "no-fpga"))]
    fpga_communication(hc, h_mem_ptr, o_mem_ptr);
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
    fn name(&self) -> &str {
        self.shape.name()
    }
    fn path(&self) -> &[usize] {
        &self.address[..]
    }
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
    fn inputs(&self) -> usize {
        self.shape.inputs()
    }
    fn outputs(&self) -> usize {
        self.shape.outputs()
    }

    // we need a different get_internal_summary function for FpgaOperator, as we need to use wrapper_internals
    // to pass initial frontier values to each ghost operator
    fn get_internal_summary(
        &mut self,
    ) -> (
        Vec<Vec<Antichain<T::Summary>>>,
        Rc<RefCell<SharedProgress<T>>>,
    ) {
        // Request the operator to be scheduled at least once.
        self.activations.borrow_mut().activate(&self.address[..]);

        // by default, we reserve a capability for each output port at `Default::default()`.
        for (_i, j) in self.ghost_indexes.iter() {
            self.shared_progress
                .borrow_mut()
                .wrapper_internals
                .get_mut(j)
                .unwrap()
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

    fn notify_me(&self) -> bool {
        self.shape.notify()
    }
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
    fn name(&self) -> &str {
        self.shape.name()
    }
    fn path(&self) -> &[usize] {
        &self.address[..]
    }

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
    fn inputs(&self) -> usize {
        self.shape.inputs()
    }
    fn outputs(&self) -> usize {
        self.shape.outputs()
    }

    // announce internal topology as fully connected, and hold all default capabilities.
    fn get_internal_summary(
        &mut self,
    ) -> (
        Vec<Vec<Antichain<T::Summary>>>,
        Rc<RefCell<SharedProgress<T>>>,
    ) {
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

    fn notify_me(&self) -> bool {
        self.shape.notify()
    }
}

/// Wrapper to run on FPGA
pub trait FpgaWrapper<S: Scope> {
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

        let mut vec_builder_filter = vec![];
        for i in 0..NUMBER_OF_FILTER_OPERATORS {
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

        let mut vec_builder_map = vec![];
        for i in 0..NUMBER_OF_MAP_OPERATORS {
            // CREATE MAP GHOST OPERATOR
            let mut builder_map =
                OperatorBuilder::new(format!("Map{}", i + 1).to_owned(), self.scope()); // scope comes from stream
            builder_map.set_notify(false);
            builder_map.set_shape(1, 1);

            let operator_logic_map = move |_progress: &mut SharedProgress<S::Timestamp>| false;

            let operator_map = FakeOperator {
                shape: builder_map.shape().clone(),
                address: builder_map.address().clone(),
                activations: self.scope().activations().clone(),
                logic: operator_logic_map,
                shared_progress: Rc::new(RefCell::new(SharedProgress::new(1, 1))),
                summary: builder_map.summary().to_vec(),
            };

            self.scope().add_operator_with_indices_no_path(
                Box::new(operator_map),
                builder_map.index(),
                builder_map.global(),
            );
            ghost_indexes.push((current_index, builder_map.index()));
            ghost_indexes2.push((current_index, builder_map.index()));
            current_index += 1;
            vec_builder_map.push(builder_map);
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

        let mut vector = Vec::with_capacity(8192);
        let mut vector2 = Vec::with_capacity(8192);

        let mut produced = HashMap::with_capacity(32);
        let mut consumed = HashMap::with_capacity(32);
        let mut internals = HashMap::with_capacity(32);

        let raw_logic = move |progress: &mut SharedProgress<S::Timestamp>| {
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

                unsafe {
                    let memory = (*hc).h_mem as *mut u64;
                    *memory.offset(current_length as isize) = *time;
                    current_length += 1;

                    for i in 0..borrow.len() {
                        let frontier = borrow[i].frontier();
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
                    for _i in current_length..FRONTIER_LENGTH {
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

                    for i in current_length..MAX_LENGTH {
                        *memory.offset(i as isize) = 0;
                    }

                    run(hc); // changes should be reflected in hc
                    let memory_out = (*hc).o_mem as *mut u64;

                    for i in 0..DATA_LENGTH {
                        let val = *memory_out.offset(i as isize) as u64;
                        let shifted_val = val >> 1;
                        if val != 0 {
                            vector2.push(shifted_val);
                        }
                    }

                    for (i, j) in ghost_indexes.iter() {
                        let consumed_index = (PROGRESS_START_INDEX + 4 * i) as isize;
                        let produced_index = (PROGRESS_START_INDEX + 4 * i + 1) as isize;
                        let internals_index_1 = (PROGRESS_START_INDEX + 4 * i + 2) as isize;
                        let internals_index_2 = (PROGRESS_START_INDEX + 4 * i + 3) as isize;

                        let consumed_value = *memory_out.offset(consumed_index) as i64;
                        let produced_value = *memory_out.offset(produced_index) as i64;
                        let internals_time = *memory_out.offset(internals_index_1) >> 1 as u64;
                        let internals_value = *memory_out.offset(internals_index_2) as i64;

                        consumed.insert(*j, consumed_value);
                        internals.insert(*j, (internals_time, internals_value));
                        produced.insert(*j, produced_value);
                    }
                }

                //let start6 = Instant::now();
                output_wrapper.session(time).give_vec(&mut vector2);

                for (_i, j) in ghost_indexes.iter() {
                    let mut cb = ChangeBatch::new_from(time.clone(), *consumed.get(j).unwrap());
                    let mut cb1 = ChangeBatch::new_from(time.clone(), *produced.get(j).unwrap());
                    let mut cb2 = ChangeBatch::new_from(
                        internals.get(j).unwrap().0 as u64,
                        internals.get(j).unwrap().1 as i64,
                    );
                    cb.drain_into(&mut progress.wrapper_consumeds.get_mut(j).unwrap()[0]);
                    cb1.drain_into(&mut progress.wrapper_produceds.get_mut(j).unwrap()[0]);
                    cb2.drain_into(&mut progress.wrapper_internals.get_mut(j).unwrap()[0]);
                }
            }

            if !has_data {
                let mut current_length = 0;

                unsafe {
                    let memory = (*hc).h_mem as *mut u64;
                    *memory.offset(current_length as isize) = 0;
                    current_length += 1;

                    for i in 0..borrow.len() {
                        let frontier = borrow[i].frontier();
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

                    for i in current_length..MAX_LENGTH {
                        *memory.offset(i as isize) = 0;
                    }

                    run(hc);

                    let memory_out = (*hc).o_mem as *mut u64;

                    for i in 0..DATA_LENGTH {
                        let val = *memory_out.offset(i as isize) as u64;
                        let shifted_val = val >> 1;
                        if val != 0 {
                            vector2.push(shifted_val);
                        }
                    }

                    for (i, j) in ghost_indexes.iter() {
                        let consumed_index = (PROGRESS_START_INDEX + 4 * i) as isize;
                        let produced_index = (PROGRESS_START_INDEX + 4 * i + 1) as isize;
                        let internals_index_1 = (PROGRESS_START_INDEX + 4 * i + 2) as isize;
                        let internals_index_2 = (PROGRESS_START_INDEX + 4 * i + 3) as isize;

                        let consumed_value = *memory_out.offset(consumed_index) as i64;
                        let produced_value = *memory_out.offset(produced_index) as i64;
                        let internals_time = *memory_out.offset(internals_index_1) >> 1 as u64;
                        let internals_value = *memory_out.offset(internals_index_2) as i64;

                        consumed.insert(*j, consumed_value);
                        internals.insert(*j, (internals_time, internals_value));
                        produced.insert(*j, produced_value);
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
            }

            vector.clear();
            vector2.clear();
            produced.clear();
            consumed.clear();
            internals.clear();
            output_wrapper.cease();

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

        for builder_map in vec_builder_map {
            ghost_operators.push(builder_map.index());
        }

        builder_wrapper.set_notify(false);
        let operator = FpgaOperator {
            shape: builder_wrapper.shape().clone(),
            address: builder_wrapper.address().clone(),
            activations: self.scope().activations().clone(),
            logic: raw_logic,
            shared_progress: Rc::new(RefCell::new(SharedProgress::new_ghosts(
                builder_wrapper.shape().inputs(),
                builder_wrapper.shape().outputs(),
                ghost_operators.clone(),
            ))),
            summary: builder_wrapper.summary().to_vec(),
            ghost_indexes: ghost_indexes2,
        };

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
