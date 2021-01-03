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

//#[path = "../../../hardware.rs"]
//pub mod hardware;

use std::ffi::c_void;
use std::collections::HashMap;

/*#[repr(C)]
///gg
pub struct HardwareCommon {
    fd: u32,
    cnfg_reg: * mut c_void,
    ctrl_reg: * mut c_void,
    buffer: * mut c_void,
    hMem: * mut i64
}

unsafe impl Send for HardwareCommon{}
unsafe impl Sync for HardwareCommon{}

#[link(name = "fpgalibrary")]
extern "C" {
    fn run(hc: * mut HardwareCommon, input: * mut u64) -> * mut u64;
}*/


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

    //TODO: add here number of operators?
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

    // announce internal topology as fully connected, and hold all default capabilities.
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

        // Request the operator to be scheduled at least once.
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
    fn fpga_wrapper(&self/*, hc: * mut HardwareCommon*/) -> Stream<S, u64>;

}


// return value should be the value of the last operator

impl<S: Scope> FpgaWrapper<S> for Stream<S, u64> {


    fn fpga_wrapper(&self/*, hc: * mut HardwareCommon*/) -> Stream<S, u64> {

        // this should correspond to the way the data will be read on the fpga
        let mut ghost_indexes = Vec::new();
        let mut ghost_indexes2 = Vec::new();
        let mut current_index = 0;

        // создание второстепенного оператора
        //он никогда не вызовется но значения для него будут положены в progress tracking

        //--------------------------------
        let mut builder_filter = OperatorBuilder::new("Filter".to_owned(), self.scope()); // scope comes from stream
        builder_filter.set_notify(false);
        builder_filter.set_shape(1, 1);
        //let mut input = PullCounter::new(builder_filter.new_input(self, Pipeline)); // builder.new_input -> creates new Input and new input connection in builder_raw.rs
        //let tee: Tee<<S as ScopeParent>::Timestamp, D> = builder_filter.new_output_without_stream();
        // this stream is returned every time, Rust will probably complain.
        // create new_output_connection function without returning the stream?
        //let mut output = PushBuffer::new(PushCounter::new(tee));

        // создание главного оператора
        //--------------
        // we can initialize data for every operator this way




        // it is good that we allocted index here, this helps when we see structures in timely which rely on the order of children
        // actually one awful thing is that edges are indexed by operator indexes in the subgraph. It means that there are no possibility
        // to skip indexes when creating child operators
        //  it would be better  to make a map in order to allow wholes in indexes

        let operator_logic =
         move |progress: &mut SharedProgress<S::Timestamp>| { false};

        let operator = FakeOperator {
            shape: builder_filter.shape().clone(),
            address: builder_filter.address().clone(),
            activations: self.scope().activations().clone(),
            logic: operator_logic,
            shared_progress: Rc::new(RefCell::new(SharedProgress::new(1, 1))),
            summary: builder_filter.summary().to_vec(),
        };


        self.scope().add_operator_with_indices_no_path(Box::new(operator), builder_filter.index(), builder_filter.global());
        ghost_indexes.push((current_index, builder_filter.index()));
        ghost_indexes2.push((current_index, builder_filter.index()));


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
                while let Some(message) = input_wrapper.next() {
                    let (time, data) = match message.as_ref_or_mut() {
                        RefOrMut::Ref(reference) => (&reference.time, RefOrMut::Ref(&reference.data)),
                        RefOrMut::Mut(reference) => (&reference.time, RefOrMut::Mut(&mut reference.data)),
                    };
                    data.swap(&mut vector);
                    // I should call my fpga function here with vector as an input

                    //let fpga_data;
                    let mut produced = HashMap::new();
                    let mut consumed = HashMap::new();
                    let mut internals = HashMap::new();
                    let mut info_length =  2 + ghost_indexes.len() + 3 * ghost_indexes.len();
                    let mut all_length = (((info_length + vector.len()) / 8) + 1) as i64;
                    let mut current_length = 0;
                    let mut max_length = 16;
                    let mut data_start_index = 0;
                    let mut progress_start_index = 0;

                    unsafe {

                        input_vector.push(all_length);
                        current_length += 1;
                        data_start_index += 1;
                        progress_start_index += 1;
                        input_vector.push(1/*time as i64*/);
                        current_length += 1;
                        data_start_index += 1;
                        progress_start_index += 1;
                        //TODO: what from antichain should we push
                        // For now I know that frontier wil be one number , when this won't be the case,
                        // we could again just allocate the max number of elements it can take.
                        for i in 0 .. borrow.len() {
                            let frontier = borrow[i].borrow().frontier();
                            for val in frontier.iter() {
                                input_vector.push(9/*val as i64*/);
                                current_length += 1;
                                data_start_index += 1;
                                progress_start_index += 1;
                            }
                        }
                        for i in  0 .. ghost_indexes.len() {
                            input_vector.push(0);
                            input_vector.push(0);
                            input_vector.push(0);
                            current_length += 3;
                            data_start_index += 3;
                        }
                        for val in vector.iter() {
                            input_vector.push(*val as i64);
                            current_length += 1;
                        }

                        for i in current_length .. max_length {
                            input_vector.push(0);
                        }

                        for (i, elem) in vector.iter().enumerate() {
                            println!("{} input element = {}", i, *elem);
                        }

                        println!("all_length = {}", all_length);
                        println!("current_length = {}", current_length);
                        println!("data_start_index = {}", data_start_index);
                        println!("progress_start_index = {}", progress_start_index);


                        /*fpga_data = run(hc, vector.as_mut_ptr());// changes should be reflected in hc
                        let output = Vec::from_raw_parts(fpga_data, all_length as usize, all_length as usize);
                        */
                        let mut output = Vec::new();
                        output.push(input_vector[0]); // all length
                        output.push(input_vector[1]); // time
                        output.push(input_vector[2]); // frontier
                        output.push(1); // consumed
                        output.push(0); // internals
                        output.push(1); // produceds
                        output.push(3); // data
                        output.push(0); output.push(0); output.push(0);
                        output.push(0); output.push(0); output.push(0);
                        output.push(0); output.push(0); output.push(0);

                        for (i, elem) in output.iter().enumerate() {
                            println!("{} output element = {}", i, elem);
                        }

                        for i in data_start_index .. max_length {
                            let val = output[i] as u64;
                            let shifted_val = val >> 1;
                            println!("shifted val = {}", shifted_val);
                            if val != 0 {
                                vector2.push(shifted_val);
                            }
                        }
                        for (i, j) in ghost_indexes.iter() {
                            println!("consumed = {}", output[progress_start_index]);
                            println!("internal = {}", output[progress_start_index + 1]);
                            println!("produced = {}", output[progress_start_index + 2]);

                            consumed.insert(*j, output[progress_start_index] as i64);
                            internals.insert(*j, (output[progress_start_index + 1]) as i64);
                            produced.insert(*j, (output[progress_start_index + 2]) as i64);

                        }
                    }


                    output_wrapper.session(time).give_vec(&mut vector2);

                    for (i, j) in ghost_indexes.iter() {
                        let mut cb = ChangeBatch::new_from(time.clone(), *consumed.get(j).unwrap());
                        let mut cb1 = ChangeBatch::new_from(time.clone(), *produced.get(j).unwrap());
                        cb.drain_into(&mut progress.wrapper_consumeds.get_mut(j).unwrap()[0]);
                        cb1.drain_into(&mut progress.wrapper_produceds.get_mut(j).unwrap()[0]);
                    }

                }
                output_wrapper.cease();

                // move batches of internal changes.
                /*let self_internal_borrow = self_internal.borrow_mut();
                for index in 0 .. self_internal_borrow.len() {
                    let mut borrow = self_internal_borrow[index].borrow_mut();
                    progress.internals[index].extend(borrow.drain());
                }*/

                // extract what we know about progress from the input and output adapters.
                //input_wrapper.consumed().borrow_mut().drain_into(&mut progress.consumeds[0]);
                //output_wrapper.inner().produced().borrow_mut().drain_into(&mut progress.produceds[0]);

                //let tt = 5 as S::Timestamp;
                //let mut cb = ChangeBatch::new_from(time, 10);
                //cb.drain_into(&mut progress.consumeds[0]);

                false
            };

        let mut ghost_operators = Vec::new();
        let mut ghost_edges = Vec::new();

        ghost_operators.push(builder_filter.index());

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

       /* let mut builder_wrapper = OperatorBuilder::new("FPGA".to_owned(), self.scope()); // scope comes from stream
        let mut input = PullCounter::new(builder_wrapper.new_input(self, Pipeline)); // builder.new_input -> creates new Input and new input connection in builder_raw.rs
        let (tee, stream) = builder_wrapper.new_output();
        // this stream is returned every time, Rust will probably complain.
        // create new_output_connection function without returning the stream?
        let mut output = PushBuffer::new(PushCounter::new(tee));

        logic
        // we can still work without normal input / output handles like in probe, these handles just add maybe unnecessary checks
        // all operators return stream

        let operator = OperatorCore {
            shape: self.shape,
            address: self.address,
            activations: self.scope().activations().clone(),
            logic,
            shared_progress: Rc::new(RefCell::new(SharedProgress::new(inputs, outputs))),
            summary: self.summary,
        };

        self.scope().add_operator_with_indices(Box::new(operator), self.index, self.global);


        stream;


        // запушить апдейты надо в свои структуры  shared_progress
*/
    }
}

// damn, OperatorCore is private Type, we can't use it to create operator structure
// need to create our own type

// schedule будет вызывать наш оператор с logic

// в идеале мы должны иметь shared_progress для каждого оператора , но результаты будут приходить только в первый оператор
// а потом он должен распределять апейты по структурам других операторов.

// consumed - можем получить из input, produced можем получить из output
// frontier - просто new Mutable Antichain
// internal - можем тоже инициализировать как массив


//если мы оставляем operator core то отдельный schedule нам не нужен
// в логике оператора будет прописано куда чего он кладет

// все операторы в начличии и в графе но в path не добавляются
/*impl<T:Timestamp> Schedule for Operator<T, D> {

    fn name(&self) -> &str { &self.name }

    fn path(&self) -> &[usize] { &self.address[..] }

    fn schedule(&mut self) -> bool {
        let shared_progress = &mut *self.shared_progress.borrow_mut();
        (self.logic)(shared_progress);
        self.progress.borrow_mut().drain_into(&mut shared_progress.internals[0]);
        self.messages.borrow_mut().drain_into(&mut shared_progress.produceds[0]);
        false
    }
}*/

// готовый schedule вызывается с той логикой, которую предлагает окончательный метод build,
// это логика по загонке frontier внутрь оператора, по исполнению логики непосредственно оператора,
// и потом по помещению результатов в shared Progress. Логика нам нужна, а вот загонка не очень

// проблема оставить текущий build в том, что тогда будут операторы внутри fpga включаться в path

// то есть я не могу вызвать builder.build потому что это приведет

// все операторы типа unary, binary они используют сначала builder_rc, который около основной логики
// оператора вставляет логику по загонке значений, есть операторы которые сразу используют raw_builder,
// там не вставляется ниакой логики вокруг а просто добавляется оператор в список.
// но я не могу просто добавить оператор в список потому что он будет добавлен в path -> то что исполняется.

// можно самой вручную добавить , add_operator_with_indices в конце концов трансформируется в add_child
// добавить еще одну функцию add_operator_with_indicies и еще одну add-child


// schedule берет ту логику которую мы подали в operatorcore, поэтому можно написать свою логику и подать ее в
// в OperatorCore
// logic это по идее просто лямбда функция
// тогда можно не писать schedule


// нужно создать new_input, new_output
// в probe используется build_raw как раз потому что он там хотел создать свою логику внутри build, так как builder_raw
// не навешивает ничего лишнего
/*impl<T:Timestamp> Operate<T> for Operator<T, D> {

    fn inputs(&self) -> usize { 0 }
    fn outputs(&self) -> usize { 1 }

    fn get_internal_summary(&mut self) -> (Vec<Vec<Antichain<<T as Timestamp>::Summary>>>, Rc<RefCell<SharedProgress<T>>>) {
        self.shared_progress.borrow_mut().internals[0].update(T::minimum(), self.copies as i64);
        (Vec::new(), self.shared_progress.clone())
    }

    fn notify_me(&self) -> bool { false }
}*/

// connection ля new_input - вектор из ANtichain длиной в outputs
// то есть это connection этого input к каждому output

// connection в new_output - вектор из Antichain длиной в inputs
// connection этого output к каждому input

// shape.outputs векторов добавили в вектор

//fn add_edge(&self, source: Source, target: Target) {
//    self.subgraph.borrow_mut().connect(source, target);
//}

// we can use this function as this just added edges to an intermediate edges array in subgraph.
// This array is usd afterwards to only add edges to the progress tracking builder,
// what we actually need

