//! Starts a timely dataflow execution from configuration information and per-worker logic.

use crate::communication::{Params, initialize_from, Configuration, Allocator, allocator::AllocateBuilder, WorkerGuards};
use crate::dataflow::scopes::Child;
use crate::worker::Worker;

/*struct Params {
    rounds: i64,
    data: i64,
    operators: i64
}*/

use libc::c_int;
use libc::{MAP_FAILED, MAP_FIXED, MAP_SHARED, PROT_READ, PROT_WRITE};
use std::convert::TryInto;
use std::ffi::c_void;

#[cfg(feature = "xdma")]
use crate::dataflow::operators::fpga_wrapper_xdma::HardwareCommon;
#[cfg(feature = "eci")]
use crate::dataflow::operators::fpga_wrapper_eci::HardwareCommon;

#[cfg(feature = "xdma")]
#[link(name = "xdma_shim")]
extern "C" {
    fn initialize(input_size: i64, output_size: i64) -> * const HardwareCommon;
    fn closeHardware(hc: * const HardwareCommon);
}

#[cfg(feature = "eci")]
extern "C" {
    fn open(pathname: *const libc::c_char, flags: c_int) -> c_int;
    fn get_nprocs() -> i32;

static SIZE: usize = 0x1000;

/// Gets a file descriptor to the FPGA memory section
#[cfg(feature = "eci")]
fn get_fpga_mem() -> i32 {
    let path = std::ffi::CString::new("/dev/fpgamem").unwrap();

    // Calling libc function here as I couldn't get Rust native to work
    let fd = unsafe { open(path.as_ptr(), libc::O_RDWR) };
    fd
}

/// Takes a file descriptor to mmap into
#[cfg(feature = "eci")]
fn mmap_wrapper(fd: c_int, no_cpus: c_int) -> Result<*mut c_void, std::io::Error> {
    let area = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            SIZE * no_cpus as usize,
            PROT_READ | PROT_WRITE,
            MAP_SHARED | MAP_FIXED,
            fd,
            0,
        )
    };

    if area == MAP_FAILED {
        Err(std::io::Error::last_os_error())
    }
    else {
        Ok(area as *mut c_void)
    }
}

/// Runs munmap over the given pointer
#[cfg(feature = "eci")]
fn munmap_wrapper(area: *mut c_void, no_cpus: libc::size_t) {
    unsafe { libc::munmap(area, 0x10000000000 * no_cpus) };
}

/// Allocate resources needed for computation
#[cfg(feature = "eci")]
fn initialize() -> *const HardwareCommon {
    let area;
    #[cfg(feature = "no-fpga")]
    {
        let red = "\x1b[31m";
        let reset = "\x1b[0m";
        println!("{red}====== Warning: No FPGA! ======{reset}");
        area = std::ptr::null_mut();
    }

    #[cfg(not(feature = "no-fpga"))]
    {
        let nprocs = unsafe { get_nprocs() };
        let fd = get_fpga_mem();
        area = mmap_wrapper(fd, nprocs).unwrap();
    }

    let hc: HardwareCommon = HardwareCommon {
        area,
    };

    // This is some magic to get the proper type
    let boxed_hc = Box::into_raw(Box::new(hc)) as *const HardwareCommon;
    boxed_hc
}

/// Free allocated resources again
#[cfg(feature = "eci")]
fn close_hardware(hc: *const HardwareCommon) {
    // Unmap mmap'd memory area
    let nprocs = unsafe { get_nprocs() };
    let area = unsafe { (*hc).area };
    munmap_wrapper(area, nprocs.try_into().unwrap());

    // As for the malloc'd memory:
    // We simply leak the malloc'd memory as it gets free'd on exit anyway.
}
}

/// Executes a single-threaded timely dataflow computation.
///
/// The `example` method takes a closure on a `Scope` which it executes to initialize and run a
/// timely dataflow computation on a single thread. This method is intended for use in examples,
/// rather than programs that may need to run across multiple workers.
///
/// The `example` method returns whatever the single worker returns from its closure.
/// This is often nothing, but the worker can return something about the data it saw in order to
/// test computations.
///
/// The method aggressively unwraps returned `Result<_>` types.
///
/// # Examples
///
/// The simplest example creates a stream of data and inspects it.
///
/// ```rust
/// use timely::dataflow::operators::{ToStream, Inspect};
///
/// timely::example(|scope| {
///     (0..10).to_stream(scope)
///            .inspect(|x| println!("seen: {:?}", x));
/// });
/// ```
///
/// This next example captures the data and displays them once the computation is complete.
///
/// More precisely, the example captures a stream of events (receiving batches of data,
/// updates to input capabilities) and displays these events.
///
/// ```rust
/// use timely::dataflow::operators::{ToStream, Inspect, Capture};
/// use timely::dataflow::operators::capture::Extract;
///
/// let data = timely::example(|scope| {
///     (0..10).to_stream(scope)
///            .inspect(|x| println!("seen: {:?}", x))
///            .capture()
/// });
///
/// // the extracted data should have data (0..10) at timestamp 0.
/// assert_eq!(data.extract()[0].1, (0..10).collect::<Vec<_>>());
/// ```
pub fn example<T, F>(func: F) -> T
where
    T: Send+'static,
    F: FnOnce(&mut Child<Worker<crate::communication::allocator::thread::Thread>,u64>)->T+Send+Sync+'static
{
    crate::execute::execute_directly(|worker| worker.dataflow(|scope| func(scope)))
}


/// Executes a single-threaded timely dataflow computation.
///
/// The `execute_directly` constructs a `Worker` and directly executes the supplied
/// closure to construct and run a timely dataflow computation. It does not create any
/// worker threads, and simply uses the current thread of control.
///
/// The closure may return a result, which will be returned from the computation.
///
/// # Examples
/// ```rust
/// use timely::dataflow::operators::{ToStream, Inspect};
///
/// // execute a timely dataflow using three worker threads.
/// timely::execute_directly(|worker| {
///     worker.dataflow::<(),_,_>(|scope| {
///         (0..10).to_stream(scope)
///                .inspect(|x| println!("seen: {:?}", x));
///     })
/// });
/// ```
pub fn execute_directly<T, F>(func: F) -> T
where
    T: Send+'static,
    F: FnOnce(&mut Worker<crate::communication::allocator::thread::Thread>)->T+Send+Sync+'static
{
    let alloc = crate::communication::allocator::thread::Thread::new();
    let mut worker = crate::worker::Worker::new(alloc);
    let result = func(&mut worker);
    while worker.step_or_park(None) { }
    result
}

/// Executes a timely dataflow from a configuration and per-communicator logic.
///
/// The `execute` method takes a `Configuration` and spins up some number of
/// workers threads, each of which execute the supplied closure to construct
/// and run a timely dataflow computation.
///
/// The closure may return a `T: Send+'static`.  The `execute` method returns
/// immediately after initializing the timely computation with a result
/// containing a `WorkerGuards<T>` (or error information), which can be joined
/// to recover the result `T` values from the local workers.
///
/// *Note*: if the caller drops the result of `execute`, the drop code will
/// block awaiting the completion of the timely computation. If the result
/// of the method is not captured it will be dropped, which gives the experience
/// of `execute` blocking; to regain control after `execute` be sure to
/// capture the results and drop them only when the calling thread has no
/// other work to perform.
///
/// # Examples
/// ```rust
/// use timely::dataflow::operators::{ToStream, Inspect};
///
/// // execute a timely dataflow using three worker threads.
/// timely::execute(timely::Configuration::Process(3), |worker| {
///     worker.dataflow::<(),_,_>(|scope| {
///         (0..10).to_stream(scope)
///                .inspect(|x| println!("seen: {:?}", x));
///     })
/// }).unwrap();
/// ```
///
/// The following example demonstrates how one can extract data from a multi-worker execution.
/// In a multi-process setting, each process will only receive those records present at workers
/// in the process.
///
/// ```rust
/// use std::sync::{Arc, Mutex};
/// use timely::dataflow::operators::{ToStream, Inspect, Capture};
/// use timely::dataflow::operators::capture::Extract;
///
/// // get send and recv endpoints, wrap send to share
/// let (send, recv) = ::std::sync::mpsc::channel();
/// let send = Arc::new(Mutex::new(send));
///
/// // execute a timely dataflow using three worker threads.
/// timely::execute(timely::Configuration::Process(3), move |worker| {
///     let send = send.lock().unwrap().clone();
///     worker.dataflow::<(),_,_>(move |scope| {
///         (0..10).to_stream(scope)
///                .inspect(|x| println!("seen: {:?}", x))
///                .capture_into(send);
///     });
/// }).unwrap();
///
/// // the extracted data should have data (0..10) thrice at timestamp 0.
/// assert_eq!(recv.extract()[0].1, (0..30).map(|x| x / 3).collect::<Vec<_>>());
/// ```
pub fn execute<T, F>(params: Params, mut config: Configuration, func: F) -> Result<WorkerGuards<T>,String>
where
    T:Send+'static,
    F: Fn(&mut Worker<Allocator>, *const HardwareCommon, Params)->T+Send+Sync+'static {

    if let Configuration::Cluster { ref mut log_fn, .. } = config {

        *log_fn = Box::new(|events_setup| {

            let mut result = None;
            if let Ok(addr) = ::std::env::var("TIMELY_COMM_LOG_ADDR") {

                use ::std::net::TcpStream;
                use crate::logging::BatchLogger;
                use crate::dataflow::operators::capture::EventWriter;

                eprintln!("enabled COMM logging to {}", addr);

                if let Ok(stream) = TcpStream::connect(&addr) {
                    let writer = EventWriter::new(stream);
                    let mut logger = BatchLogger::new(writer);
                    result = Some(crate::logging_core::Logger::new(
                        ::std::time::Instant::now(),
                        ::std::time::Duration::default(),
                        events_setup,
                        move |time, data| logger.publish_batch(time, data)
                    ));
                }
                else {
                    panic!("Could not connect to communication log address: {:?}", addr);
                }
            }
            result
        });
    }

    let (allocators, other) = config.try_build()?;

    initialize_from(allocators, other, params, move |allocator| {

        let hwcommon;
        unsafe {
            hwcommon = initialize(params.data * 8 + 64, params.data * 8 + 64);
        }

        println!("Param 1 = {} (initialize from)", params.rounds);
        println!("Param 2 = {} (initialize from)", params.data);
        println!("Param 3 = {} (initialize from)", params.operators);

        let mut worker = Worker::new(allocator);

        // If an environment variable is set, use it as the default timely logging.
        if let Ok(addr) = ::std::env::var("TIMELY_WORKER_LOG_ADDR") {

            use ::std::net::TcpStream;
            use crate::logging::{BatchLogger, TimelyEvent};
            use crate::dataflow::operators::capture::EventWriter;

            if let Ok(stream) = TcpStream::connect(&addr) {
                let writer = EventWriter::new(stream);
                let mut logger = BatchLogger::new(writer);
                worker.log_register()
                    .insert::<TimelyEvent,_>("timely", move |time, data|
                        logger.publish_batch(time, data)
                    );
            }
            else {
                panic!("Could not connect logging stream to: {:?}", addr);
            }
        }

        let result = func(&mut worker, hwcommon, params);
        while worker.step_or_park(None) { }

        unsafe {
            closeHardware(hwcommon);
        }
        result
    })
}

/// Executes a timely dataflow from supplied arguments and per-communicator logic.
///
/// The `execute` method takes arguments (typically `std::env::args()`) and spins up some number of
/// workers threads, each of which execute the supplied closure to construct and run a timely
/// dataflow computation.
///
/// The closure may return a `T: Send+'static`.  The `execute_from_args` method
/// returns immediately after initializing the timely computation with a result
/// containing a `WorkerGuards<T>` (or error information), which can be joined
/// to recover the result `T` values from the local workers.
///
/// *Note*: if the caller drops the result of `execute_from_args`, the drop code
/// will block awaiting the completion of the timely computation.
///
/// The arguments `execute_from_args` currently understands are:
///
/// `-w, --workers`: number of per-process worker threads.
///
/// `-n, --processes`: number of processes involved in the computation.
///
/// `-p, --process`: identity of this process; from 0 to n-1.
///
/// `-h, --hostfile`: a text file whose lines are "hostname:port" in order of process identity.
/// If not specified, `localhost` will be used, with port numbers increasing from 2101 (chosen
/// arbitrarily).
///
/// # Examples
///
/// ```rust
/// use timely::dataflow::operators::{ToStream, Inspect};
///
/// // execute a timely dataflow using command line parameters
/// timely::execute_from_args(std::env::args(), |worker| {
///     worker.dataflow::<(),_,_>(|scope| {
///         (0..10).to_stream(scope)
///                .inspect(|x| println!("seen: {:?}", x));
///     })
/// }).unwrap();
/// ```
/// ```ignore
/// host0% cargo run -- -w 2 -n 4 -h hosts.txt -p 0
/// host1% cargo run -- -w 2 -n 4 -h hosts.txt -p 1
/// host2% cargo run -- -w 2 -n 4 -h hosts.txt -p 2
/// host3% cargo run -- -w 2 -n 4 -h hosts.txt -p 3
/// ```
/// ```ignore
/// % cat hosts.txt
/// host0:port
/// host1:port
/// host2:port
/// host3:port
/// ```
pub fn execute_from_args<I, T, F>(iter1: I, iter: I, func: F) -> Result<WorkerGuards<T>,String>
    where I: Iterator<Item=String>,
          T:Send+'static,
          F: Fn(&mut Worker<Allocator>, *const HardwareCommon, Params)->T+Send+Sync+'static, {


    let args_vector: Vec<String> = iter1.collect();

    let param1 = args_vector[1].parse().expect("Invalid age");
    let param2 = args_vector[2].parse().expect("Invalid age");
    let param3 = args_vector[3].parse().expect("Invalid age");

    println!("Param 1 = {}", param1);
    println!("Param 2 = {}", param2);
    println!("Param 3 = {}", param3);

    let configuration = Configuration::from_args(iter)?;

    let params: Params = Params {rounds: param1, data: param2, operators: param3};
    
    execute(params, configuration, func)
}

/// Executes a timely dataflow from supplied allocators and logging.
///
/// Refer to [`execute`](fn.execute.html) for more details.
///
/// ```rust
/// use timely::dataflow::operators::{ToStream, Inspect};
///
/// // execute a timely dataflow using command line parameters
/// let (builders, other) = timely::Configuration::Process(3).try_build().unwrap();
/// timely::execute::execute_from(builders, other, |worker| {
///     worker.dataflow::<(),_,_>(|scope| {
///         (0..10).to_stream(scope)
///                .inspect(|x| println!("seen: {:?}", x));
///     })
/// }).unwrap();
/// ```
pub fn execute_from<A, T, F>(builders: Vec<A>, others: Box<dyn ::std::any::Any+Send>, func: F) -> Result<WorkerGuards<T>,String>
where
    A: AllocateBuilder+'static,
    T: Send+'static,
    F: Fn(&mut Worker<<A as AllocateBuilder>::Allocator>)->T+Send+Sync+'static {

    let params: Params = Params {rounds: 0, data: 0, operators: 0};
    initialize_from(builders, others, params, move |allocator| {
        let mut worker = Worker::new(allocator);
        let result = func(&mut worker);
        while worker.step_or_park(None) { }
        result
    })
}
