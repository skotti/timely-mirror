obj-m += enzian_memory.o

all: emd mb_enzian mem mem2

emd:
	make -C /lib/modules/$(shell uname -r)/build M=$(PWD) modules

clean:
	make -C /lib/modules/$(shell uname -r)/build M=$(PWD) clean
	rm -f mb_enzian mem mem2

mb_enzian: mb_enzian.c enzian_memory.h
	gcc -o mb_enzian mb_enzian.c -Wall -O2 -lpthread -mtune=thunderx

mem: mem.c
	gcc -o mem mem.c -Wall -O2

mem2: mem2.c
	gcc -o mem2 mem2.c -Wall -O2
