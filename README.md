# Timely Dataflow #

Timely dataflow is a low-latency cyclic dataflow computational model, introduced in the paper [Naiad: a timely dataflow system](http://dl.acm.org/citation.cfm?id=2522738). This project is an extended and more modular implementation of timely dataflow in Rust.


# Timely Dataflow on heterogeneous system #

This version of Timely Dataflow is my attempt to turn this engine into a fully heterogeneous system. There is a System Verilog backend part as well which is not part of this repository.

Branch **timely_all_together**: branch with all the up-to-date changes.

Branch **async**: explores asynchronous execution, which is not part of the flow in timely_all_together branch.

![system_design drawio](https://github.com/user-attachments/assets/96bf3c0a-4d3e-4919-9261-b20f3e477036)

On this diagram I present the current view on the system: there is a possibility to offload a graph or an individual operator and use different means of communication depending on the chosen data batch size.

Key words: stream processing, FPGA, heterogeneous computing, stream processing on FPGA
