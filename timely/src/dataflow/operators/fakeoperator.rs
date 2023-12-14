/// Ghost operator, resides on the FPGA side

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

