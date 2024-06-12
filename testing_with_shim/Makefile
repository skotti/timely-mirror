CC = gcc
CFLAGS = -Wall -Werror -O2 -fPIC
LDFLAGS = -shared

SRC = xdma_shim.c
HEADER = xdma_shim.h
LIBNAME = libxdma_shim.so

all: $(LIBNAME)

$(LIBNAME): $(SRC)
	$(CC) $(CFLAGS) $(LDFLAGS) -o $(LIBNAME) $(SRC)

clean:
	rm -f $(LIBNAME)
