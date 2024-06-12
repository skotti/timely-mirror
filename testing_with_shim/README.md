To test stuff:

1) make

2) gcc -o output\_executable test.c -L/home/enzian/xdma\_shim -lxdma\_shim -Wall -Werror

3) sudo LD\_LIBRARY\_PATH=$LD\_LIBRARY\_PATH:/home/enzian/xdma\_shim/ ./output\_executable
