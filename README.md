# Timely Dataflow #

Timely dataflow is a low-latency cyclic dataflow computational model, introduced in the paper [Naiad: a timely dataflow system](http://dl.acm.org/citation.cfm?id=2522738). This project is an extended and more modular implementation of timely dataflow in Rust.


# Timely Dataflow on heterogeneous system #

This version in particular is trying to turn this engine into a fully heterogeneous system. There is a System Verilog backend part as well. 

The branch where currently all the resources are is timely_all_together.

There is also an async branch, that explores async execution but it is outside of the main flow for now.

Key words: stream processing, FPGA, heterogeneous computing, stream processing on FPGA