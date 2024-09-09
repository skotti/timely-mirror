//! Extension methods for `Stream` based on record-by-record transformation.

use std::convert::TryInto;
use crate::Data;
use crate::dataflow::{Stream, Scope};
use crate::dataflow::channels::pact::Pipeline;
use crate::dataflow::operators::generic::operator::Operator;


use libc::{c_int, open};
use libc::{MAP_FAILED, MAP_FIXED, MAP_SHARED, PROT_READ, PROT_WRITE};
use std::ffi::c_void;

static CACHE_LINE_SIZE: usize = 128;


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


/// Extension trait for `Stream`.
pub trait BloomHardware<S: Scope> {
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
    fn bloom_hardware< L: FnMut(u8)->u8+'static>(&self, logic: L) -> Stream<S, u8>;
}

impl<S: Scope> BloomHardware<S> for Stream<S, u8> {
    fn bloom_hardware<L: FnMut(u8)->u8+'static>(&self, mut logic: L) -> Stream<S, u8> {
        let mut vector = Vec::new();
        let mut vector2 = Vec::with_capacity(16);

        let path = std::ffi::CString::new("/dev/fpgamem").unwrap();

        // Calling libc function here as I couldn't get Rust native to work
        let fd = unsafe { open(path.as_ptr(), libc::O_RDWR) };

        let mut offset_1 = 0;
        let mut offset_2 = 16; let area = unsafe {
            libc::mmap(
                0x100000000000 as *mut c_void,
                0x10000000000 as usize,
                PROT_READ | PROT_WRITE,
                MAP_SHARED | MAP_FIXED,
                fd,
                0,
            )
        };


        let area = unsafe { area } as *mut u64;
        let cache_line_1 = unsafe { std::slice::from_raw_parts_mut(area.offset(offset_1.try_into().unwrap()), CACHE_LINE_SIZE as usize) };
        let cache_line_2 = unsafe {
            std::slice::from_raw_parts_mut(
                area.offset(offset_2.try_into().unwrap()),
                CACHE_LINE_SIZE as usize,
            )
        };


        self.unary(Pipeline, "Map", move |_,_| move |input, output| {
            input.for_each(|time, data| {
                data.swap(&mut vector);


                if vector.len() != 0  {
                    for i in 0..16 {
                        cache_line_1[i] = vector[i] as u64;
                    }

                    dmb();

                    for i in 0..16 {
                        vector2[i] = cache_line_2[i] as u8;
                    }

                    dmb();
                }

                output.session(&time).give_vec(&mut vector2);

            });
        })
    }
}
