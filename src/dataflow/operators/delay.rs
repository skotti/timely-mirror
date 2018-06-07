//! Operators acting on timestamps to logically delay records

use std::collections::HashMap;

use Data;
use order::PartialOrder;
use dataflow::channels::pact::Pipeline;
use dataflow::{Stream, Scope};
use dataflow::operators::generic::operator::Operator;

/// Methods to advance the timestamps of records or batches of records.
pub trait Delay<G: Scope, D: Data> {
    /// Advances the timestamp of records using a supplied function.
    ///
    /// The function *must* advance the timestamp; the operator will test that the
    /// new timestamp is greater or equal to the old timestamp, and will assert if
    /// it is not.
    ///
    /// #Examples
    ///
    /// The following example takes the sequence `0..10` at time `RootTimestamp(0)`
    /// and delays each element `i` to time `RootTimestamp(i)`.
    ///
    /// ```
    /// use timely::dataflow::operators::{ToStream, Delay, Operator};
    /// use timely::dataflow::channels::pact::Pipeline;
    /// use timely::progress::timestamp::RootTimestamp;
    ///
    /// timely::example(|scope| {
    ///     (0..10).to_stream(scope)
    ///            .delay(|data, time| RootTimestamp::new(*data))
    ///            .sink(Pipeline, "example", |input| {
    ///                input.for_each(|time, data| {
    ///                    println!("data at time: {:?}", time);
    ///                });
    ///            });
    /// });
    /// ```
    fn delay(&self, impl Fn(&D, &G::Timestamp)->G::Timestamp+'static) -> Self;
    /// Advances the timestamp of batches of records using a supplied function.
    ///
    /// The function *must* advance the timestamp; the operator will test that the
    /// new timestamp is greater or equal to the old timestamp, and will assert if
    /// it is not. The batch version does not consult the data, and may only view
    /// the timestamp itself.
    ///
    /// #Examples
    ///
    /// The following example takes the sequence `0..10` at time `RootTimestamp(0)`
    /// and delays each batch (there is just one) to time `RootTimestamp(1)`.
    ///
    /// ```
    /// use timely::dataflow::operators::{ToStream, Delay, Operator};
    /// use timely::dataflow::channels::pact::Pipeline;
    /// use timely::progress::timestamp::RootTimestamp;
    ///
    /// timely::example(|scope| {
    ///     (0..10).to_stream(scope)
    ///            .delay_batch(|time| RootTimestamp::new(time.inner + 1))
    ///            .sink(Pipeline, "example", |input| {
    ///                input.for_each(|time, data| {
    ///                    println!("data at time: {:?}", time);
    ///                });
    ///            });
    /// });
    /// ```
    fn delay_batch(&self, impl Fn(&G::Timestamp)->G::Timestamp+'static) -> Self;
}

impl<G: Scope, D: Data> Delay<G, D> for Stream<G, D> {
    fn delay(&self, func: impl Fn(&D, &G::Timestamp)->G::Timestamp+'static) -> Stream<G, D> {
        let mut elements = HashMap::new();
        let mut vector = Vec::new();
        self.unary_notify(Pipeline, "Delay", vec![], move |input, output, notificator| {
            input.for_each(|time, data| {
                data.swap(&mut vector);
                for datum in vector.drain(..) {
                    let new_time = func(&datum, &time);
                    assert!(time.time().less_equal(&new_time));
                    elements.entry(new_time.clone())
                            .or_insert_with(|| { notificator.notify_at(time.delayed(&new_time)); Vec::new() })
                            .push(datum);
                }
            });

            // for each available notification, send corresponding set
            notificator.for_each(|time,_,_| {
                if let Some(mut data) = elements.remove(&time) {
                    output.session(&time).give_iterator(data.drain(..));
                }
            });
        })
    }

    fn delay_batch(&self, func: impl Fn(&G::Timestamp)->G::Timestamp+'static) -> Stream<G, D> {
        let mut elements = HashMap::new();
        self.unary_notify(Pipeline, "Delay", vec![], move |input, output, notificator| {
            input.for_each(|time, data| {
                let new_time = func(&time);
                assert!(time.time().less_equal(&new_time));
                elements.entry(new_time.clone())
                        .or_insert_with(|| { notificator.notify_at(time.delayed(&new_time)); Vec::new() })
                        .push(data.replace(Vec::new()));
            });

            // for each available notification, send corresponding set
            notificator.for_each(|time,_,_| {
                if let Some(mut datas) = elements.remove(&time) {
                    for mut data in datas.drain(..) {
                        output.session(&time).give_vec(&mut data);
                    }
                }
            });
        })
    }
}
