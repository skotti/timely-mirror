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

extern "C" {
    fn malloc(size: usize) -> *mut std::ffi::c_void;
}

// Various parameters
const NUMBER_OF_INPUTS: usize = 8; // make sure to sync with caller (e.g. `hello_fpga.rs`)
const TR_SIZE_OUT: isize = 144 * 8;
const NUMBER_OF_FILTER_OPERATORS: usize = 10;
const NUMBER_OF_MAP_OPERATORS: usize = 1;
const OPERATOR_COUNT: usize = NUMBER_OF_FILTER_OPERATORS + NUMBER_OF_MAP_OPERATORS;
const PARAM: usize = 8;
const PARAM_OUTPUT: usize = 8;
const FRONTIER_PARAM: usize = 3;
const FRONTIER_LENGTH: usize = FRONTIER_PARAM * 8;
const MAX_LENGTH_IN: usize = PARAM * 8 + FRONTIER_PARAM * 8;
const DATA_LENGTH: usize = PARAM_OUTPUT * 8;
const PROGRESS_START_INDEX: usize = PARAM_OUTPUT * 8;
const PROGRESS_OUTPUT: usize = 10; //ceil((#operators * 4)/8)
const MAX_LENGTH_OUT: usize = PARAM_OUTPUT * 8 + PROGRESS_OUTPUT * 8;
const TIME_INDEX: usize = MAX_LENGTH_OUT;

#[derive(Debug)]
#[repr(C)]
/// Data structure to store FPGA related data
pub struct HardwareCommon {
    /// Input memory
    pub h_mem: *mut c_void,
    /// Output memory
    pub o_mem: *mut c_void,

    /// Outstanding queue
    pub i_queue: std::collections::VecDeque<*mut u64>,

    /// Input queue
    pub h_queue: std::collections::VecDeque<*mut u64>,

    /// Output queue
    pub o_queue: std::collections::VecDeque<*mut u64>,

    /// Memory queue
    pub mem_queue: std::collections::VecDeque<*mut u64>,

    /// time queue?
    pub t_queue: std::collections::VecDeque<std::time::Instant>,

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
                        // assert_eq!(std::u64::MAX, input_arr[0]);
    offset += 1;

    //
    let same_value = input_arr[offset];
    for i in 0..OPERATOR_COUNT {
        let i = i + offset;
        assert!(0 == input_arr[i] || 1 == input_arr[i] || 3 == input_arr[i]);
        assert_eq!(same_value, input_arr[i]); // values should be the same across
    }
    offset += OPERATOR_COUNT;

    //
    offset += OPERATOR_COUNT + 1;

    //
    let same_value = input_arr[offset];
    for i in 0..NUMBER_OF_INPUTS {
        let i = i + offset;
        assert!(0 == input_arr[i] || 43 == input_arr[i]);
        assert_eq!(same_value, input_arr[i]); // values should be the same across
    }

    //
    let first_val: u64;
    let second_val: u64;
    if same_value == 43 {
        first_val = 22529;
        second_val = 24;
    } else {
        first_val = 0;
        second_val = 0;
    }
    // Cast buffer ptr to array
    let output_arr: &mut [u64] = unsafe { std::slice::from_raw_parts_mut(output_ptr, 144) };
    let mut my_offset = 0;

    let unknown_value1 = 24;
    let unknown_value2 = 40;
    // 1...1 - number of inputs times
    for _i in 0..unknown_value1 {
        output_arr[my_offset] = first_val;
        my_offset += 1;
    }
    for _i in 0..unknown_value2 {
        output_arr[my_offset] = 0;
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

    // Mark as valid
    unsafe { *output_ptr.offset(TR_SIZE_OUT / 8 - 1) = 0 };
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

/// Checks whether the invalid bit is set or not
/// If the invalid bit is gone, that means the FPGA has written to the location
fn overwritten(mem: *mut u64) -> bool {
    let result = unsafe { *mem.offset(TR_SIZE_OUT / 8 - 1) != u64::MAX };
    result
}

/// Adjusted version of the original C code.
/// Where the original version used some memory queue to store allocated chunks of memory,
/// this simply runs `malloc`
/// The reason for this change being that we don't deal with the same slowdown from memory alloc as the original code
fn get_mem() -> *mut u64 {
    let size_of_u64 = 8;
    unsafe { malloc(size_of_u64 * (TR_SIZE_OUT as usize)) as *mut u64 }
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

/// Writes to the first cache line
fn write_to_memory(val: [u64; 16], area: *mut std::ffi::c_void) {
    // Treat as `uint64_t *`
    let area = area as *mut u64;

    for i in 0..16 as usize {
        unsafe { *area.offset(i.try_into().unwrap()) = val[i] };
    }
    dmb();
}

/// Reads from the second cache line
fn read_from_memory(area: *mut std::ffi::c_void) -> [u64; 16] {
    let mut res: [u64; 16] = [0; 16];

    // Treat as `uint64_t *`
    let area = area as *mut u64;

    // Read
    for i in 0..16 {
        res[i] = unsafe { *(area.offset((16 + i).try_into().unwrap())) };
    }
    dmb();

    res
}

/// Communicates to FPGA via cache line using [`2fast2forward`](https://gitlab.inf.ethz.ch/PROJECT-Enzian/fpga-sources/enzian-applications/2fast2forward)
fn fpga_communication(hc: *const HardwareCommon) {
    let val: [u64; 16] = [420; 16]; // Using the same value for each element due to a quirk in provided bitstream reordering some values

    // Get pointer to memory
    let area = unsafe { (*hc).area };

    // Write to cache line
    write_to_memory(val, area);

    // Read from other cache line
    let res = read_from_memory(area);

    // Use the input we gave to FPGA to calculate the output we expect
    // The expected output being the number left shifted and the LSB set to `1`.
    let mut expected_result: [u64; 16] = [0; 16];
    for i in 0..val.len() {
        let val = val[i];
        // Perform the left shift and set the lowest bit to 1
        let res = (val << 1) | 1;
        expected_result[i] = res
    }

    // Debug prints
    // dbg!(val);
    // dbg!(expected_result);
    // dbg!(res);

    // Check result
    // Based on current implementation
    assert_eq!(expected_result, res);
}

/// Sends data to FPGA
fn send_input(hc: *const HardwareCommon, mut input: Vec<u64>) {
    let i_queue = unsafe { &mut (*(hc as *mut HardwareCommon)).i_queue };
    let h_queue = unsafe { &mut (*(hc as *mut HardwareCommon)).h_queue };
    let o_queue = unsafe { &mut (*(hc as *mut HardwareCommon)).o_queue };

    if !o_queue.is_empty() {
        todo!();
    }

    let h_mem;
    // If outstanding queue is empty
    // Queue new input directly
    if i_queue.is_empty() {
        // no outstanding inputs, can send new input
        h_mem = get_mem();

        // why div by 8?
        for i in 0..MAX_LENGTH_IN / 8 {
            unsafe { *h_mem.offset(i.try_into().unwrap()) = input[i] };
        }
    } else {
        // If outstanding queue is non-empty, get top element from outstanding queue
        // and enqueue current input into outstanding queue
        h_mem = i_queue.pop_front().unwrap();
        let i_mem = get_mem();
        for i in 0..MAX_LENGTH_IN / 8 {
            // why div by 8?

            unsafe { *i_mem.offset(i.try_into().unwrap()) = input[i] };
        }
        i_queue.push_back(i_mem);
    }

    let o_mem_ptr = get_mem();

    // Mark as unready
    unsafe { *o_mem_ptr.offset(TR_SIZE_OUT / 8 - 1) = u64::MAX };

    // Enqueue input and output memory pointers
    h_queue.push_back(h_mem);
    o_queue.push_back(o_mem_ptr);

    let h_mem_ptr = input.as_mut_ptr();
    generate_fpga_output(h_mem_ptr, o_mem_ptr);

    #[cfg(not(feature = "no-fpga"))]
    fpga_communication(hc);
}

/// Receives FPGA response
fn check_output(hc: *const HardwareCommon, output: *mut u64) -> bool {
    let o_queue = unsafe { &mut (*(hc as *mut HardwareCommon)).o_queue };
    let h_queue = unsafe { &mut (*(hc as *mut HardwareCommon)).h_queue };
    let i_queue = unsafe { &mut (*(hc as *mut HardwareCommon)).i_queue };

    let mut send_again: bool = false;
    if o_queue.is_empty() && i_queue.is_empty() {
        return false;
    }
    if !i_queue.is_empty() {
        if o_queue.is_empty() {
            send_again = true;
        } else {
            todo!();
        }
    }

    if send_again {
        todo!();
    }
    let o_mem = o_queue.pop_front().unwrap();
    if !overwritten(o_mem) {
        return false;
    }

    // dbg!("before");
    // dbg!(size_out);
    // dbg!(o_mem);
    // dbg!(unsafe { *o_mem });
    // read_memory_chunk_u64(o_mem);
    unsafe { ptr::copy_nonoverlapping(o_mem, output, (TR_SIZE_OUT / 8) as usize) };

    let h_mem = h_queue.pop_front().unwrap();
    // output[size_out/8] = h_mem[0]; // <--- why?
    unsafe { *output.offset(TR_SIZE_OUT / 8) = *h_mem.offset(0) };

    true
}

/// Wrapper operator to store ghost operators
struct FpgaOperator<T, L>
where
    T: Timestamp,
    L: FnMut(&mut SharedProgress<T>) -> u64 + 'static,
{
    shape: OperatorShape,
    address: Vec<usize>,
    collector_address: Vec<usize>,
    logic: L,
    shared_progress: Rc<RefCell<SharedProgress<T>>>,
    activations: Rc<RefCell<Activations>>,
    summary: Vec<Vec<Antichain<T::Summary>>>,

    ghost_indexes: Vec<(usize, usize)>,
    counter: Rc<RefCell<u64>>,
}
impl<T, L> Schedule for FpgaOperator<T, L>
where
    T: Timestamp,
    L: FnMut(&mut SharedProgress<T>) -> u64 + 'static,
{
    fn name(&self) -> &str {
        self.shape.name()
    }
    fn path(&self) -> &[usize] {
        &self.address[..]
    }
    fn schedule(&mut self) -> bool {
        let shared_progress = &mut *self.shared_progress.borrow_mut();
        let counter = &mut *self.counter.borrow_mut();
        let inputs = (self.logic)(shared_progress);
        *counter += inputs;

        // activates FpgaCollector
        self.activations
            .borrow_mut()
            .activate(&self.collector_address[..]);
        false
    }
}

impl<T, L> Operate<T> for FpgaOperator<T, L>
where
    T: Timestamp,
    L: FnMut(&mut SharedProgress<T>) -> u64 + 'static,
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

/// Collector operator to collect ouputs
struct FpgaCollector<T, L>
where
    T: Timestamp,
    L: FnMut(&mut SharedProgress<T>) -> usize + 'static,
{
    shape: OperatorShape,
    address: Vec<usize>,
    logic: L,
    shared_progress: Rc<RefCell<SharedProgress<T>>>,
    activations: Rc<RefCell<Activations>>,
    summary: Vec<Vec<Antichain<T::Summary>>>,
    counter: Rc<RefCell<u64>>,
}

impl<T, L> Schedule for FpgaCollector<T, L>
where
    T: Timestamp,
    L: FnMut(&mut SharedProgress<T>) -> usize + 'static,
{
    fn name(&self) -> &str {
        self.shape.name()
    }
    fn path(&self) -> &[usize] {
        &self.address[..]
    }
    fn schedule(&mut self) -> bool {
        //let start = Instant::now();
        let shared_progress = &mut *self.shared_progress.borrow_mut();
        let counter = &mut *self.counter.borrow_mut();
        let consumed = (self.logic)(shared_progress) as u64;
        if *counter - consumed > 0 {
            *counter -= consumed;
            self.activations.borrow_mut().activate(&self.address[..]);
        } else {
            *counter = 0;
        }
        *counter != 0
    }
}

impl<T, L> Operate<T> for FpgaCollector<T, L>
where
    T: Timestamp,
    L: FnMut(&mut SharedProgress<T>) -> usize + 'static,
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
        //self.activations.borrow_mut().activate(&self.address[..]);

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

        let mut vector = Vec::with_capacity(MAX_LENGTH_IN);
        let ghost_i = ghost_indexes.to_vec();

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
            let mut inputs = 0;

            while let Some(message) = input_wrapper.next() {
                has_data = true;
                inputs += 1;
                let (time_1, data) = match message.as_ref_or_mut() {
                    RefOrMut::Ref(reference) => (&reference.time, RefOrMut::Ref(&reference.data)),
                    RefOrMut::Mut(reference) => {
                        (&reference.time, RefOrMut::Mut(&mut reference.data))
                    }
                };
                data.swap(&mut vector);

                let mut current_length = 0;

                let mut input = Vec::new();
                input.push(*time_1);
                current_length += 1;

                for i in 0..borrow.len() {
                    let frontier = borrow[i].frontier();
                    if frontier.len() == 0 {
                        input.push(0 as u64);
                        current_length += 1;
                    } else {
                        for val in frontier.iter() {
                            input.push(((*val << 1) | 1u64) as u64);
                            current_length += 1;
                        }
                    }
                }

                for _i in current_length..FRONTIER_LENGTH {
                    input.push(0 as u64);
                    current_length += 1;
                }

                if vector.len() == 0 {
                    input.push(0 as u64);
                    current_length += 1;
                } else {
                    for val in vector.iter() {
                        input.push(((*val << 1) | 1u64) as u64);
                        current_length += 1;
                    }
                }

                for _i in current_length..MAX_LENGTH_IN {
                    input.push(0 as u64);
                }

                send_input(hc, input);
                vector.clear();
            }

            if !has_data {
                let mut current_length = 0;

                let mut input = Vec::new();
                inputs += 1;
                input.push(u64::MAX);
                current_length += 1;

                for i in 0..borrow.len() {
                    let frontier = borrow[i].frontier();
                    if frontier.len() == 0 {
                        input.push(0 as u64);
                        current_length += 1;
                    } else {
                        for val in frontier.iter() {
                            input.push(((*val << 1) | 1u64) as u64);
                            current_length += 1;
                        }
                    }
                }

                for _i in current_length..FRONTIER_LENGTH {
                    input.push(0 as u64);
                    current_length += 1;
                }

                for _i in current_length..MAX_LENGTH_IN {
                    input.push(0 as u64);
                }

                send_input(hc, input);
            }
            inputs
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

        // create collector operator
        let mut builder_collector = OperatorBuilder::new("Collector".to_owned(), self.scope());
        builder_collector.set_notify(false);

        let shared_progress = Rc::new(RefCell::new(SharedProgress::new_ghosts(
            builder_wrapper.shape().inputs(),
            builder_wrapper.shape().outputs(),
            ghost_operators.clone(),
        )));
        let counter = Rc::new(RefCell::new(0 as u64));

        builder_wrapper.set_notify(false);
        let operator = FpgaOperator {
            shape: builder_wrapper.shape().clone(),
            address: builder_wrapper.address().clone(),
            collector_address: builder_collector.address().clone(),
            activations: self.scope().activations().clone(),
            logic: raw_logic,
            shared_progress: Rc::clone(&shared_progress),
            summary: builder_wrapper.summary().to_vec(),
            ghost_indexes: ghost_indexes2,
            counter: Rc::clone(&counter),
        };

        let collector_logic = move |progress: &mut SharedProgress<S::Timestamp>| {
            let mut vector2 = Vec::with_capacity(DATA_LENGTH);
            let mut produced = HashMap::with_capacity(32);
            let mut consumed = HashMap::with_capacity(32);
            let mut internals = HashMap::with_capacity(32);
            let mut o_counter = 0;

            let mut output = vec![0; MAX_LENGTH_OUT + 1];
            // let mut output = vec![0; (TR_SIZE_OUT as usize) + 1];
            // dbg!(output.len());
            // dbg!();
            while check_output(hc, output.as_mut_ptr()) {
                o_counter += 1;

                for i in 0..DATA_LENGTH {
                    let val = output[i] as u64;
                    let shifted_val = val >> 1;
                    if val != 0 {
                        vector2.push(shifted_val);
                    }
                }

                for (i, j) in ghost_i.iter() {
                    let consumed_value = output[PROGRESS_START_INDEX + 4 * i] as i64;
                    let produced_value = output[PROGRESS_START_INDEX + 4 * i + 1] as i64;
                    let internals_time = (output[PROGRESS_START_INDEX + 4 * i + 2] >> 1) as u64;
                    let internals_value = output[PROGRESS_START_INDEX + 4 * i + 3] as i64;

                    consumed.insert(*j, consumed_value);
                    internals.insert(*j, (internals_time, internals_value));
                    produced.insert(*j, produced_value);
                }
                let time = output[TIME_INDEX];
                let has_data = time != u64::MAX;
                if has_data {
                    output_wrapper.session(&time).give_vec(&mut vector2);

                    for (_i, j) in ghost_i.iter() {
                        let mut cb = ChangeBatch::new_from(time.clone(), *consumed.get(j).unwrap());
                        let mut cb1 =
                            ChangeBatch::new_from(time.clone(), *produced.get(j).unwrap());
                        let mut cb2 = ChangeBatch::new_from(
                            internals.get(j).unwrap().0 as u64,
                            internals.get(j).unwrap().1 as i64,
                        );
                        cb.drain_into(&mut progress.wrapper_consumeds.get_mut(j).unwrap()[0]);
                        cb1.drain_into(&mut progress.wrapper_produceds.get_mut(j).unwrap()[0]);
                        cb2.drain_into(&mut progress.wrapper_internals.get_mut(j).unwrap()[0]);
                    }
                } else {
                    let id_wrap = ghost_i[ghost_i.len() - 1].1;

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
                        cb1.drain_into(
                            &mut progress.wrapper_produceds.get_mut(&id_wrap).unwrap()[0],
                        );
                        cb2.drain_into(
                            &mut progress.wrapper_internals.get_mut(&id_wrap).unwrap()[0],
                        );
                    }
                }
                output_wrapper.cease();
                produced.clear();
                consumed.clear();
                internals.clear();
            }
            o_counter
        };

        let collector = FpgaCollector {
            shape: builder_collector.shape().clone(),
            address: builder_collector.address().clone(),
            activations: self.scope().activations().clone(),
            logic: collector_logic,
            shared_progress: Rc::clone(&shared_progress),
            summary: builder_collector.summary().to_vec(),
            counter: Rc::clone(&counter),
        };

        // add fpga operator to scope
        self.scope().add_operator_with_indices(
            Box::new(operator),
            builder_wrapper.index(),
            builder_wrapper.global(),
        );

        // add fpga collector to scope
        self.scope().add_operator_with_indices(
            Box::new(collector),
            builder_collector.index(),
            builder_collector.global(),
        );

        // we also need to create a map from ghost to wrapper
        self.scope().add_fpga_operator(
            builder_wrapper.index(),
            ghost_operators.clone(),
            ghost_edges.clone(),
        );

        self.scope().add_fpga_collector(
            builder_collector.index(),
            ghost_operators.clone(),
            ghost_edges.clone(),
        );

        return stream_wrapper;
    }
}
