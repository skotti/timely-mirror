//! Extension methods for `Stream` based on record-by-record transformation.

use crate::Data;
use crate::dataflow::{Stream, Scope};
use crate::dataflow::channels::pact::Pipeline;
use crate::dataflow::operators::generic::operator::Operator;

/// Extension trait for `Stream`.
pub trait Bloom<S: Scope, D: Data> {
    /// Consumes each element of the stream and yields a new element.
    ///
    /// # Examples
    /// ```
    /// use timely::dataflow::operators::{ToStream, Map, Inspect};
    ///
    /// timely::example(|scope| {
    ///     (0..10).to_stream(scope)
    ///            .map(|x| x + 1)
    ///            .inspect(|x| println!("seen: {:?}", x));
    /// });
    /// ```
    fn bloom<D2: Data, L: FnMut(D)->D2+'static>(&self, logic: L) -> Stream<S, D2>;
}

impl<S: Scope, D: Data> Map<S, D> for Stream<S, D> {
    fn bloom<D2: Data, L: FnMut(D)->D2+'static>(&self, mut logic: L) -> Stream<S, D2> {
        let mut vector = Vec::new();
        self.unary(Pipeline, "Map", move |_,_| move |input, output| {
            input.for_each(|time, data| {
                data.swap(&mut vector);
                let hash_seeds = u32x8::from_array([0, 1, 2, 3, 4, 5, 6, 7]);
                let mut hashes = hash_seeds;
                let mut bloom = [0u8; NUM_BLOOM_BYTES];
                let mut byte_counter = 0;
                let mut item_counter = 0;
                let mut output_idx = 0;


                for &byte in vector.iter() {
                    let input_broadcast = u32x8::splat(byte as u32);
                    hashes += input_broadcast;
                    hashes += hashes << 10;
                    hashes ^= hashes >> 6;

                    byte_counter += 1;
                    if byte_counter == ITEM_BYTES {
                        hashes += hashes << 3;
                        hashes ^= hashes >> 11;
                        hashes += hashes << 15;

                        let cells = (hashes >> 3) & u32x8::splat((NUM_BLOOM_BYTES - 1) as u32);
                        let vals = u32x8::splat(1).shl(hashes & u32x8::splat(7));
                        for j in 0..NUM_HASHES {
                            let cell = cells[j] as usize;
                            let val = vals[j] as u8;
                            bloom[cell] |= val;
                        }

                     }
                }

                output.session(&time).give_iterator(vector.drain(..).map(|x| logic(x)));
            });
        })
    }
}
