//! Extension methods for `Stream` based on record-by-record transformation.
#![feature(portable_simd)]
use std::simd::u32x8;

use crate::Data;
use crate::dataflow::{Stream, Scope};
use crate::dataflow::channels::pact::Pipeline;
use crate::dataflow::operators::generic::operator::Operator;

/// Extension trait for `Stream`.
pub trait Bloom<S: Scope> {
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
    fn bloom<L: FnMut(u8)->u8+'static>(&self, logic: L) -> Stream<S, u8>;
}

impl<S: Scope> Bloom<S> for Stream<S, u8> {
    fn bloom< L: FnMut(u8)->u8+'static>(&self, mut logic: L) -> Stream<S, u8> {
        let mut vector = Vec::new();

        let one_vec: u32x8 = u32x8::splat(1);
        let seven_vec: u32x8 = u32x8::splat(7);
        let ten_vec: u32x8 = u32x8::splat(10);
        let six_vec: u32x8 = u32x8::splat(6);
        let three_vec: u32x8 = u32x8::splat(3);
        let eleven_vec: u32x8 = u32x8::splat(11);
        let fifi_vec: u32x8 = u32x8::splat(15);
        let bloom_bytes= u32x8::splat(128 as u32);


        self.unary(Pipeline, "Map", move |_,_| move |input, output| {
            input.for_each(|time, data| {
                data.swap(&mut vector);
                let hash_seeds = u32x8::from_array([0, 1, 2, 3, 4, 5, 6, 7]);
                let mut hashes = hash_seeds;
                let mut bloom = [0u8; 128];
                let mut byte_counter = 0;
                let mut item_counter = 0;
                let mut output_idx = 0;


                for &byte in vector.iter() {
                    let input_broadcast = u32x8::splat( byte as u32);
                    hashes += input_broadcast;
                    hashes += hashes << ten_vec;
                    hashes ^= hashes >> six_vec;

                    byte_counter += 1;
                    if byte_counter == 128 {
                        hashes += hashes << three_vec;
                        hashes ^= hashes >> eleven_vec;
                        hashes += hashes << fifi_vec;

                        let cells = (hashes >> three_vec) & bloom_bytes;
                        let vals = one_vec << (hashes & seven_vec);

                        for j in 0..8 {
                            bloom[cells[j] as usize] |= vals[j] as u8;
                        }


                    }
                }

                output.session(&time).give_iterator(vector.drain(..).map(|x| logic(x)));
            });
        })
    }
}
